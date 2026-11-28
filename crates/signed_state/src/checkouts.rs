use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Error;
use gpui::{App, AppContext, Context, Entity, Global, Subscription};
use settings::{CheckoutRecord, CheckoutsSettings, SettingsStore};
use signed_core::{Announcement, RepoAddr};
use signed_git::Repo;
use utils::same_repo_url;

use crate::backend::{Backend, BackendEvent};
use crate::git_store::Mirrors;
use crate::local_repos::LocalReposStore;
use crate::refresh::{RefreshGate, RefreshRequest};
use crate::repos::RepoListStore;

const REFRESH_DEBOUNCE: Duration = Duration::from_millis(300);

struct GlobalCheckoutsStore(Entity<CheckoutsStore>);

impl Global for GlobalCheckoutsStore {}

struct Remembered {
    path: PathBuf,
    addr: RepoAddr,
    last_used: u64,
}

impl Remembered {
    fn resolve_associations<'a, A>(
        remembered: &[Remembered],
        scanned: &[(PathBuf, Option<String>, Option<String>)],
        announcements: A,
    ) -> HashMap<RepoAddr, Vec<PathBuf>>
    where
        A: IntoIterator<Item = &'a Announcement>,
    {
        let announcements: Vec<&Announcement> = announcements.into_iter().collect();
        let mut out: HashMap<RepoAddr, Vec<PathBuf>> = HashMap::new();

        let mut sorted: Vec<&Remembered> = remembered.iter().collect();
        sorted.sort_by_key(|record| std::cmp::Reverse(record.last_used));
        for record in sorted {
            let paths = out.entry(record.addr.clone()).or_default();
            if !paths.contains(&record.path) {
                paths.push(record.path.clone());
            }
        }

        for (path, origin, root) in scanned {
            for announcement in &announcements {
                let url_match = origin.as_deref().is_some_and(|origin| {
                    announcement
                        .clone
                        .iter()
                        .any(|url| same_repo_url(origin, url.as_str()))
                });

                let euc_match = root
                    .as_deref()
                    .is_some_and(|root| announcement.euc.as_deref() == Some(root));

                if url_match || euc_match {
                    let paths = out.entry(announcement.addr()).or_default();
                    if !paths.contains(path) {
                        paths.push(path.clone());
                    }
                }
            }
        }

        out
    }
}

pub struct CheckoutsStore {
    by_repo: HashMap<RepoAddr, Vec<PathBuf>>,
    refresh: RefreshGate,
    debounce_pending: bool,
    checkouts_settings: CheckoutsSettings,
    _subscriptions: Vec<Subscription>,
}

impl CheckoutsStore {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalCheckoutsStore>().0.clone()
    }

    pub(crate) fn set_global(entity: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalCheckoutsStore(entity));
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        let mut checkouts_settings = CheckoutsSettings::default();

        if !cfg!(target_arch = "wasm32") {
            let settings = SettingsStore::global(cx);
            let local = LocalReposStore::global(cx);
            let repos = RepoListStore::global(cx);
            let backend = Backend::global(cx);

            checkouts_settings = settings.read(cx).settings().checkouts.clone();

            subscriptions.push(cx.observe(&settings, |this, settings, cx| {
                let checkouts = settings.read(cx).settings().checkouts.clone();
                if this.checkouts_settings == checkouts {
                    return;
                }
                // Only edits to the checkouts section affect the derived state
                this.checkouts_settings = checkouts;
                this.refresh(cx);
            }));

            subscriptions.push(cx.observe(&local, |this, _local, cx| {
                this.refresh(cx);
            }));

            subscriptions.push(cx.observe(&repos, |this, _repos, cx| {
                this.refresh(cx);
            }));

            // Another identity's repositories must not keep the old associations alive.
            subscriptions.push(cx.subscribe(&backend, |this, _backend, event, cx| {
                if matches!(event, BackendEvent::SignerChanged) {
                    let had_state = !this.by_repo.is_empty();
                    this.by_repo.clear();
                    if had_state {
                        cx.notify();
                    }

                    this.refresh(cx);
                }
            }));
        }

        if !cfg!(target_arch = "wasm32") {
            let weak = cx.weak_entity();
            cx.defer(move |cx| {
                weak.update(cx, |this, cx| {
                    this.refresh(cx);
                })
                .ok();
            });
        }

        Self {
            by_repo: HashMap::new(),
            refresh: RefreshGate::default(),
            debounce_pending: false,
            checkouts_settings,
            _subscriptions: subscriptions,
        }
    }

    pub fn record(&mut self, path: PathBuf, addr: RepoAddr, cx: &mut Context<Self>) {
        if cfg!(target_arch = "wasm32") {
            return;
        }

        let settings = SettingsStore::global(cx);
        let addr_str = addr.to_string();

        let last_used = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        settings.update(cx, |settings, cx| {
            settings.edit(
                |s| {
                    s.checkouts
                        .records
                        .retain(|r| !(r.path == path && r.addr == addr_str));
                    s.checkouts.records.push(CheckoutRecord {
                        path,
                        addr: addr_str,
                        last_used,
                    });
                },
                cx,
            );
        });
    }

    pub fn associations_of(&self, addr: &RepoAddr) -> Vec<PathBuf> {
        self.by_repo.get(addr).cloned().unwrap_or_default()
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.debounce_pending || self.refresh.request() != RefreshRequest::Schedule {
            return;
        }

        self.debounce_pending = true;

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH_DEBOUNCE).await;
            this.update(cx, |this, cx| {
                this.run_refresh(cx);
            })
        })
        .detach();
    }

    fn run_refresh(&mut self, cx: &mut Context<Self>) {
        self.debounce_pending = false;
        self.refresh.begin();

        let records = {
            let settings = SettingsStore::global(cx);
            settings.read(cx).settings().checkouts.records.clone()
        };

        let remembered: Vec<Remembered> = records
            .into_iter()
            .filter_map(|record| {
                let addr = record.addr.parse::<RepoAddr>().ok()?;
                Some(Remembered {
                    path: record.path,
                    addr,
                    last_used: record.last_used,
                })
            })
            .collect();

        let announcements = RepoListStore::global(cx).read(cx).announcements.clone();
        let scanned = LocalReposStore::global(cx).read(cx).repos.clone();
        let cache_root = Mirrors::root().canonicalize().ok();

        let work = cx.background_spawn(async move {
            let mut facts = Vec::new();
            for scanned in scanned.iter() {
                let path = &scanned.path;

                if cache_root
                    .as_ref()
                    .is_some_and(|root| path.starts_with(root))
                {
                    continue;
                }

                let (origin, root) = match Repo::open(path) {
                    Ok(repo) => (
                        repo.origin_url().ok().flatten(),
                        repo.root_commit().ok().flatten(),
                    ),
                    Err(_) => (None, None),
                };

                facts.push((path.clone(), origin, root));
            }

            let associations =
                Remembered::resolve_associations(&remembered, &facts, announcements.iter());

            associations
                .into_iter()
                .map(|(addr, paths)| (addr, paths.into_iter().filter(|p| p.is_dir()).collect()))
                .collect::<HashMap<RepoAddr, Vec<PathBuf>>>()
        });

        cx.spawn(async move |this, cx| {
            let associations = work.await;
            let again = this.update(cx, |this, cx| {
                let changed = this.by_repo != associations;
                this.by_repo = associations;

                if changed {
                    cx.notify();
                }

                this.refresh.finish()
            })?;

            if again {
                this.update(cx, |this, cx| this.refresh(cx))?;
            }

            Ok::<(), Error>(())
        })
        .detach();
    }
}
