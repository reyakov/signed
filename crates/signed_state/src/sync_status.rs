use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Error;
use gpui::{App, AppContext, Context, Entity, Global, Subscription};
use nostr_sdk::prelude::*;
use signed_core::{Deletions, Filters, RepoAddr, RepoState};
use signed_git::{Repo, RepoSyncStatus};

use crate::backend::{Backend, BackendEvent};
use crate::checkouts::CheckoutsStore;
use crate::refresh::{RefreshGate, RefreshRequest};
use crate::repos::RepoListStore;

// Nostr state changes rarely, so drift detection runs on a slow poll.
const SYNC_POLL: Duration = Duration::from_secs(60);
const MAX_SYNC_CHECKOUTS: usize = 8;

struct GlobalSyncStatusStore(Entity<SyncStatusStore>);

impl Global for GlobalSyncStatusStore {}

/// A checkout compared against the repository state published on Nostr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutSyncStatus {
    pub path: PathBuf,
    pub status: RepoSyncStatus,
}

pub struct SyncStatusStore {
    statuses: HashMap<RepoAddr, Vec<CheckoutSyncStatus>>,
    refresh: RefreshGate,
    poll_pending: bool,
    _subscriptions: Vec<Subscription>,
}

impl SyncStatusStore {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalSyncStatusStore>().0.clone()
    }

    pub(crate) fn set_global(entity: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalSyncStatusStore(entity));
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();

        if !cfg!(target_arch = "wasm32") {
            let repos = RepoListStore::global(cx);
            let checkouts = CheckoutsStore::global(cx);
            let backend = Backend::global(cx);

            subscriptions.push(cx.observe(&repos, |this, _repos, cx| {
                this.refresh(cx);
            }));

            // Checkout associations decide which paths get compared.
            subscriptions.push(cx.observe(&checkouts, |this, _checkouts, cx| {
                this.refresh(cx);
            }));

            subscriptions.push(
                cx.subscribe(&backend, |this, _backend, event, cx| match event {
                    BackendEvent::SignerChanged => {
                        this.statuses.clear();
                        cx.notify();
                        this.refresh(cx);
                    }
                    BackendEvent::RepoUpdates(_) | BackendEvent::Synced => this.refresh(cx),
                    _ => {}
                }),
            );

            let weak = cx.entity().downgrade();
            cx.defer(move |cx| {
                if let Err(error) = weak.update(cx, |this, cx| this.refresh(cx)) {
                    log::warn!("sync status store dropped before the initial refresh: {error}");
                }
            });
        }

        Self {
            statuses: HashMap::new(),
            refresh: RefreshGate::default(),
            poll_pending: false,
            _subscriptions: subscriptions,
        }
    }

    /// Latest sync status per associated checkout.
    pub fn statuses_of(&self, addr: &RepoAddr) -> Vec<CheckoutSyncStatus> {
        self.statuses.get(addr).cloned().unwrap_or_default()
    }

    /// Commits available locally but missing on Nostr.
    pub fn unsynced(&self, addr: &RepoAddr) -> usize {
        self.statuses
            .get(addr)
            .map(|list| {
                list.iter()
                    .map(|checkout| checkout.status.ahead_total)
                    .sum()
            })
            .unwrap_or(0)
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if cfg!(target_arch = "wasm32") {
            return;
        }
        if self.refresh.request() != RefreshRequest::Schedule {
            return;
        }

        let backend = Backend::global(cx);
        let Some(user) = backend.read(cx).current_user() else {
            let changed = !self.statuses.is_empty();
            self.statuses.clear();
            if changed {
                cx.notify();
            }
            return;
        };

        self.schedule_poll(cx);
        self.run_refresh(user, cx);
    }

    fn schedule_poll(&mut self, cx: &mut Context<Self>) {
        if self.poll_pending {
            return;
        }
        self.poll_pending = true;

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SYNC_POLL).await;
            this.update(cx, |this, cx| {
                this.poll_pending = false;
                this.refresh(cx);
            })
        })
        .detach();
    }

    fn run_refresh(&mut self, user: PublicKey, cx: &mut Context<Self>) {
        self.refresh.begin();

        let backend = Backend::global(cx);
        let client = backend.read(cx).client();

        let repos = RepoListStore::global(cx);
        let announcements = repos.read(cx).announcements_of(&user);
        let checkouts = CheckoutsStore::global(cx);

        let targets: Vec<(RepoAddr, Vec<PathBuf>)> = announcements
            .iter()
            .map(|announcement| {
                let addr = announcement.addr();
                let paths = checkouts.read(cx).associations_of(&addr);
                (addr, paths)
            })
            .collect();

        let work = cx.background_spawn(async move {
            let db = client.database();
            let deletion_events = db.query(Filters::deletions()).await?;
            let deletions = Deletions::from_events(deletion_events);

            let mut states: HashMap<RepoAddr, Vec<(String, String)>> = HashMap::new();
            for (addr, _) in &targets {
                let events = db.query(addr.state_filter()).await?;
                let latest = utils::latest(
                    events
                        .into_iter()
                        .filter(|event| !deletions.is_deleted(event)),
                );
                let Some(event) = latest else {
                    continue;
                };
                states.insert(addr.clone(), RepoState::parse(&event).refs);
            }

            let mut statuses: HashMap<RepoAddr, Vec<CheckoutSyncStatus>> = HashMap::new();
            for (addr, paths) in &targets {
                let Some(nostr_refs) = states.get(addr) else {
                    continue;
                };

                let mut list = Vec::new();
                for path in paths.iter().take(MAX_SYNC_CHECKOUTS) {
                    let synced =
                        Repo::try_open(path).and_then(|repo| repo.sync_status(nostr_refs).ok());

                    if let Some(status) = synced {
                        list.push(CheckoutSyncStatus {
                            path: path.clone(),
                            status,
                        });
                    }
                }

                if !list.is_empty() {
                    statuses.insert(addr.clone(), list);
                }
            }

            Ok::<_, Error>(statuses)
        });

        cx.spawn(async move |this, cx| {
            let statuses = match work.await {
                Ok(statuses) => statuses,
                Err(error) => {
                    log::warn!("failed to compute nostr sync statuses: {error}");
                    return this.update(cx, |this, _cx| {
                        this.refresh.abort();
                    });
                }
            };

            let again = this.update(cx, |this, cx| {
                if this.statuses != statuses {
                    this.statuses = statuses;
                    cx.notify();
                }
                this.refresh.finish()
            })?;

            if again {
                this.update(cx, |this, cx| this.refresh(cx))?;
            }

            Ok::<_, Error>(())
        })
        .detach();
    }
}
