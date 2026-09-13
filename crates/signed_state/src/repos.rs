use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Error;
use gpui::{App, AppContext, Context, Entity, Global, Subscription, Task};
use nostr_sdk::prelude::*;
use signed_core::{Announcement, Deletions, RepoAddr, filters, repo_addr};
use signed_git::find_git_repos;

use crate::backend::{Backend, BackendEvent};
use crate::refresh::{RefreshGate, RefreshRequest};

struct GlobalLocalReposStore(Entity<LocalReposStore>);

impl Global for GlobalLocalReposStore {}

/// Store of the git repositories discovered under a set of scan paths.
pub struct LocalReposStore {
    pub roots: Arc<Vec<PathBuf>>,
    /// Git repositories discovered under [`Self::roots`], sorted by path.
    pub repos: Arc<Vec<PathBuf>>,
    pub scanning: bool,
    scan_dirty: bool,
}

impl LocalReposStore {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalLocalReposStore>().0.clone()
    }

    pub(crate) fn set_global(entity: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalLocalReposStore(entity));
    }

    pub fn new(roots: Vec<PathBuf>, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        cx.defer(move |cx| {
            if let Err(error) = weak.update(cx, |this, cx| this.rescan(cx)) {
                log::warn!("local repos store dropped before initial scan could run: {error}");
            }
        });

        Self {
            roots: Arc::new(roots),
            repos: Arc::new(Vec::new()),
            scanning: false,
            scan_dirty: false,
        }
    }

    /// Forget a repository that has just been published to NIP-34.
    pub fn remove(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.repos = Arc::new(
            self.repos
                .iter()
                .filter(|repo| repo.as_path() != path)
                .cloned()
                .collect(),
        );
        cx.notify();
    }

    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        if self.scanning {
            self.scan_dirty = true;
            return;
        }

        if self.roots.is_empty() {
            return;
        }

        self.scanning = true;
        cx.notify();

        let roots = self.roots.clone();

        let work = cx.background_spawn(async move {
            let mut repos = Vec::new();
            for root in roots.iter() {
                repos.extend(find_git_repos(root));
            }
            repos.sort();
            repos.dedup();
            repos
        });

        let task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            let repos = work.await;
            let again = this.update(cx, |this, cx| {
                this.repos = Arc::new(repos);
                this.scanning = false;
                cx.notify();

                let dirty = this.scan_dirty;
                this.scan_dirty = false;
                dirty
            })?;

            // Scans requested while this one ran are coalesced into one follow-up scan.
            if again {
                this.update(cx, |this, cx| this.rescan(cx))?;
            }

            Ok(())
        });

        task.detach();
    }
}

/// How far back activity events count toward a repository's last activity.
const ACTIVITY_WINDOW: Duration = Duration::from_secs(90 * 86_400);

struct GlobalRepoListStore(Entity<RepoListStore>);

impl Global for GlobalRepoListStore {}

/// NIP-34 activity event counts per repository, ranking the explore list by popularity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RepoActivityCounts {
    /// Root `30611` issue events addressed to the repository.
    pub issues: u32,
    /// Root `3063` pull request events addressed to the repository.
    ///
    /// PR updates are not new PRs and do not count.
    pub pull_requests: u32,
    /// `1617` patch events addressed to the repository.
    pub commits: u32,
}

impl RepoActivityCounts {
    /// Total issues, pull requests and commits, the popularity ranking key.
    pub fn score(self) -> u32 {
        self.issues + self.pull_requests + self.commits
    }
}

/// Store listing the discovered repository announcements, newest first.
pub struct RepoListStore {
    /// Shared so views can clone the list per frame without a deep copy.
    pub announcements: Arc<Vec<Announcement>>,
    /// Latest known activity timestamp per repository.
    pub last_activity: Arc<HashMap<RepoAddr, Timestamp>>,
    /// Issues, pull requests and commits per repository.
    ///
    /// Used for the Popular ranking of the explore list.
    pub counts: Arc<HashMap<RepoAddr, RepoActivityCounts>>,
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
                BackendEvent::NostrUpdate(updates) => updates.iter().any(|update| {
                    // Deletions may target anything we list, always refresh.
                    if update.kind == Kind::EventDeletion || update.kind == Kind::RequestToVanish {
                        true
                    } else if filters::ACTIVITY_KINDS.contains(&update.kind) {
                        // Activity events are addressed to repos via `a` tags.
                        // Their author is not the repo owner, always refresh.
                        true
                    } else {
                        let is_announcement = update.kind == Kind::GitRepoAnnouncement;
                        let is_repo_state = update.kind == Kind::RepoState;
                        is_announcement || is_repo_state
                    }
                }),
                BackendEvent::Published(event) => {
                    let announcement = event.kind == Kind::GitRepoAnnouncement;

                    // Locally published deletions are already in the local database.
                    // Refresh so they take effect immediately, like relay deletions.
                    let deletion =
                        event.kind == Kind::EventDeletion || event.kind == Kind::RequestToVanish;

                    announcement || deletion
                }
                // Only a completed sync refreshes the list.
                // Progress ticks would re-scan the whole database several times
                // per sync to reveal entries incrementally.
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
            refresh: RefreshGate::default(),
            _subscription: subscription,
        }
    }

    /// The announcements of `user`, newest first.
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
            backend.sync_bootstrap(filters::all_announcements(), cx);
            // Deletion requests, NIP-09/62, must be known before any announcement is shown.
            backend.sync_bootstrap(filters::deletions(), cx);
        });
    }

    /// Re-query the local database.
    ///
    /// Runs immediately. The backend pump already batches the relay events that
    /// trigger a refresh, so no per-store debounce is needed.
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
            let filter = filters::all_announcements();
            let events = client.database().query(filter).await?;

            let deletion_events = client.database().query(filters::deletions()).await?;
            let deletions = Deletions::from_events(deletion_events);

            // Dedup and sort off the main thread.
            // Only the final list crosses back into the entity.
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

            // Last activity per repository.
            // State updates count, and all NIP-34 activity events.
            // The activity events are patches, PRs, issues and statuses.
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
                let addr = repo_addr(event.pubkey, id);
                let Some(entry) = last_activity.get_mut(&addr) else {
                    continue;
                };
                *entry = (*entry).max(event.created_at);
            }

            // Bound the activity query to a recent window.
            // Older repos fall back to their announcement or state timestamps.
            let activity_filter = Filter::new()
                .kinds(filters::ACTIVITY_KINDS)
                .since(Timestamp::now() - ACTIVITY_WINDOW);
            for event in client.database().query(activity_filter).await? {
                if deletions.is_deleted(&event) {
                    continue;
                }
                for addr in event.tags.coordinates() {
                    if addr.kind != Kind::GitRepoAnnouncement {
                        continue;
                    }
                    // Skip events for repos we do not list.
                    // The map cannot grow beyond the number of announcements.
                    let Some(entry) = last_activity.get_mut(&addr) else {
                        continue;
                    };
                    *entry = (*entry).max(event.created_at);
                }
            }

            // Popularity counts per repository, issues, pull requests and patches.
            // Unbounded, unlike the windowed activity query above, so totals are exact.
            let mut counts: HashMap<RepoAddr, RepoActivityCounts> = HashMap::new();
            let count_filter =
                Filter::new().kinds([Kind::GitIssue, Kind::GitPullRequest, Kind::GitPatch]);
            for event in client.database().query(count_filter).await? {
                if deletions.is_deleted(&event) {
                    continue;
                }
                for addr in event.tags.coordinates() {
                    // Skip events for repos we do not list.
                    // The map cannot grow beyond the number of announcements.
                    if addr.kind != Kind::GitRepoAnnouncement || !last_activity.contains_key(&addr)
                    {
                        continue;
                    }
                    let entry = counts.entry(addr).or_default();
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
                // Database errors are transient, keep the last list.
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
                cx.notify();

                this.refresh.finish()
            })?;

            // Requests that arrived while the refresh was running.
            // They are coalesced into one follow-up refresh.
            if again {
                this.update(cx, |this, cx| this.refresh(cx))?;
            }

            Ok(())
        })
        .detach();
    }
}
