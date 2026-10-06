use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Error;
use gpui::{App, AppContext, Context, Entity, Global, Subscription};
use nostr_sdk::prelude::*;
use signed_core::{Announcement, Deletions, Filters, RepoAddr, filters};

use crate::backend::{Backend, BackendEvent};
use crate::refresh::{RefreshGate, RefreshRequest};

/// How far back activity events count toward a repository's last activity.
const ACTIVITY_WINDOW: Duration = Duration::from_secs(90 * 86_400);

struct GlobalRepoListStore(Entity<RepoListStore>);

impl Global for GlobalRepoListStore {}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RepoActivityCounts {
    pub issues: u32,
    pub pull_requests: u32,
    pub commits: u32,
}

impl RepoActivityCounts {
    pub fn score(self) -> u32 {
        self.issues + self.pull_requests + self.commits
    }
}

pub struct RepoListStore {
    pub announcements: Arc<Vec<Announcement>>,
    pub last_activity: Arc<HashMap<RepoAddr, Timestamp>>,
    pub counts: Arc<HashMap<RepoAddr, RepoActivityCounts>>,
    state_synced_repos: HashSet<RepoAddr>,
    refresh: RefreshGate,
    _subscription: Subscription,
}

impl RepoListStore {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalRepoListStore>().0.clone()
    }

    pub(crate) fn set_global(entity: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalRepoListStore(entity));
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        let backend = Backend::global(cx);
        let weak = cx.entity().downgrade();

        let subscription = cx.subscribe(&backend, |this, _backend, event, cx| {
            let relevant = match event {
                BackendEvent::SignerChanged => {
                    this.state_synced_repos.clear();
                    true
                }
                BackendEvent::RepoUpdates(_) => true,
                BackendEvent::Synced => true,
                _ => false,
            };

            if relevant {
                this.refresh(cx);
            }
        });

        cx.defer(move |cx| {
            weak.update(cx, |this, cx| {
                this.subscribe_remote(cx);
                this.refresh(cx);
            })
            .ok();
        });

        Self {
            announcements: Arc::new(Vec::new()),
            last_activity: Arc::new(HashMap::new()),
            counts: Arc::new(HashMap::new()),
            state_synced_repos: HashSet::new(),
            refresh: RefreshGate::default(),
            _subscription: subscription,
        }
    }

    pub fn announcements_of(&self, user: &PublicKey) -> Vec<Announcement> {
        self.announcements
            .iter()
            .filter(|a| a.owner == *user)
            .cloned()
            .collect()
    }

    fn subscribe_remote(&mut self, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);

        backend.update(cx, |backend, cx| {
            backend.sync_bootstraps(
                vec![
                    Filters::all_announcements(),
                    Filters::all_states(),
                    // Deletion requests, NIP-09/62, must be known before any announcement is shown.
                    Filters::deletions(),
                ],
                cx,
            );
        });
    }

    fn sync_own_repo_states(&mut self, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let Some(me) = backend.read(cx).current_user() else {
            return;
        };

        let pending: Vec<(RepoAddr, Vec<RelayUrl>)> = self
            .announcements
            .iter()
            .filter(|announcement| announcement.owner == me && !announcement.relays.is_empty())
            .map(|announcement| (announcement.addr(), announcement.relays.clone()))
            .filter(|(addr, _)| !self.state_synced_repos.contains(addr))
            .collect();

        for (addr, relays) in pending {
            self.state_synced_repos.insert(addr.clone());

            backend.update(cx, |backend, cx| {
                backend.connect_repo_relays(relays, vec![addr.state_filter()], cx);
            });
        }
    }

    // Runs immediately: the backend pump already batches the relay events
    // that trigger a refresh, so no per-store debounce is needed.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refresh.request() != RefreshRequest::Schedule {
            return;
        }

        self.run_refresh(cx);
    }

    fn run_refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh.begin();

        let backend = Backend::global(cx);
        let client = backend.read(cx).client();

        let work = cx.background_spawn(async move {
            let filter = Filters::all_announcements();
            let events = client.database().query(filter).await?;

            let deletion_events = client.database().query(Filters::deletions()).await?;
            let deletions = Deletions::from_events(deletion_events);

            // Dedup and sort off the main thread; only the final list crosses
            // back into the entity.
            let mut by_repo: HashMap<RepoAddr, Announcement> = HashMap::new();

            for event in events {
                if deletions.is_deleted(&event) {
                    continue;
                }

                let Some(announcement) = Announcement::from_event(&event) else {
                    continue;
                };

                let addr = announcement.addr();

                match by_repo.get(&addr) {
                    Some(existing) if existing.created_at >= announcement.created_at => {}
                    _ => {
                        by_repo.insert(addr, announcement);
                    }
                }
            }

            let mut announcements: Vec<Announcement> = by_repo.into_values().collect();
            announcements.sort_by_key(|a| std::cmp::Reverse(a.created_at));

            let mut last_activity: HashMap<RepoAddr, Timestamp> = announcements
                .iter()
                .map(|a| (a.addr(), a.created_at))
                .collect();

            let state_filter = Filter::new().kind(Kind::RepoState);
            for event in client.database().query(state_filter).await? {
                if deletions.is_deleted(&event) {
                    continue;
                }
                let Some(id) = event.tags.identifier() else {
                    continue;
                };
                let addr = RepoAddr::new(event.pubkey, id);
                let Some(entry) = last_activity.get_mut(&addr) else {
                    continue;
                };
                *entry = (*entry).max(event.created_at);
            }

            // Older repos fall back to their announcement or state timestamps.
            let activity_filter = Filter::new()
                .kinds(filters::ACTIVITY_KINDS)
                .since(Timestamp::now() - ACTIVITY_WINDOW);

            for event in client.database().query(activity_filter).await? {
                if deletions.is_deleted(&event) {
                    continue;
                }
                for coordinate in event.tags.coordinates() {
                    if coordinate.kind != Kind::GitRepoAnnouncement {
                        continue;
                    }
                    let addr = RepoAddr::from(coordinate.clone());
                    let Some(entry) = last_activity.get_mut(&addr) else {
                        continue;
                    };
                    *entry = (*entry).max(event.created_at);
                }
            }

            // Unbounded, unlike the windowed activity query above, so totals
            // are exact.
            let mut counts: HashMap<RepoAddr, RepoActivityCounts> = HashMap::new();
            let count_filter =
                Filter::new().kinds([Kind::GitIssue, Kind::GitPullRequest, Kind::GitPatch]);

            for event in client.database().query(count_filter).await? {
                if deletions.is_deleted(&event) {
                    continue;
                }
                for coordinate in event.tags.coordinates() {
                    if coordinate.kind != Kind::GitRepoAnnouncement
                        || !last_activity.contains_key(&RepoAddr::from(coordinate.clone()))
                    {
                        continue;
                    }
                    let entry = counts.entry(RepoAddr::from(coordinate)).or_default();
                    match event.kind {
                        Kind::GitIssue => entry.issues += 1,
                        Kind::GitPullRequest => entry.pull_requests += 1,
                        Kind::GitPatch => entry.commits += 1,
                        _ => {}
                    }
                }
            }

            Ok::<_, Error>((announcements, last_activity, counts))
        });

        cx.spawn(async move |this, cx| {
            let (announcements, last_activity, counts) = match work.await {
                Ok(results) => results,
                Err(_) => {
                    return this.update(cx, |this, _cx| {
                        this.refresh.abort();
                    });
                }
            };

            let again = this.update(cx, |this, cx| {
                this.announcements = Arc::new(announcements);
                this.last_activity = Arc::new(last_activity);
                this.counts = Arc::new(counts);
                this.sync_own_repo_states(cx);
                cx.notify();

                this.refresh.finish()
            })?;

            if again {
                this.update(cx, |this, cx| this.refresh(cx))?;
            }

            Ok(())
        })
        .detach();
    }
}
