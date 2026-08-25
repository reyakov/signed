use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Error;
use gpui::{AppContext, Context, Subscription, Task};
use nostr_sdk::prelude::*;
use signed_core::{Announcement, Deletions, RepoAddr, filters, repo_addr};

use crate::backend::{Backend, BackendEvent};

/// Delay between a refresh request and the actual re-query, so bursts of
/// events (e.g. sync progress ticks) collapse into one query.
const REFRESH_DEBOUNCE: Duration = Duration::from_millis(300);

/// How far back activity events count toward a repository's last activity.
const ACTIVITY_WINDOW: Duration = Duration::from_secs(90 * 86_400);

/// Store listing repository announcements (global discovery or per-author).
pub struct RepoListStore {
    /// Shared so views can clone the list per frame without a deep copy.
    pub announcements: Arc<Vec<Announcement>>,
    /// Latest known activity timestamp per repository
    /// (announcements, state updates, patches, PRs, issues, statuses).
    pub last_activity: Arc<HashMap<RepoAddr, Timestamp>>,
    author: Option<PublicKey>,
    refreshing: bool,
    refresh_dirty: bool,
    /// A refresh is waiting out [`REFRESH_DEBOUNCE`].
    debouncing: bool,
    tasks: Vec<Task<Result<(), Error>>>,
    _subscription: Subscription,
}

impl RepoListStore {
    /// Create a store. If `author` is `None`, all announcements are listed.
    pub fn new(author: Option<PublicKey>, cx: &mut Context<Self>) -> Self {
        let backend = Backend::global(cx);

        let subscription = cx.subscribe(&backend, |this, _backend, event, cx| {
            let relevant = match event {
                BackendEvent::NostrUpdate(update) => {
                    // Deletions may target anything we list; always refresh.
                    if update.kind == Kind::EventDeletion || update.kind == Kind::RequestToVanish {
                        true
                    } else if filters::ACTIVITY_KINDS.contains(&update.kind) {
                        // Activity (patches, issues, ...) is addressed to repos via
                        // `a` tags, so its author isn't the repo owner; always refresh.
                        true
                    } else {
                        let is_announcement = update.kind == Kind::GitRepoAnnouncement;
                        let is_repo_state = update.kind == Kind::RepoState;
                        let tracked = is_announcement || is_repo_state;
                        tracked && this.author.is_none_or(|a| a == update.author)
                    }
                }
                BackendEvent::Published(event) => {
                    event.kind == Kind::GitRepoAnnouncement
                        && this.author.is_none_or(|a| a == event.pubkey)
                }
                BackendEvent::Synced | BackendEvent::SyncProgress { .. } => true,
                _ => false,
            };

            if relevant {
                this.refresh(cx);
            }
        });

        let mut store = Self {
            announcements: Arc::new(Vec::new()),
            last_activity: Arc::new(HashMap::new()),
            author,
            refreshing: false,
            refresh_dirty: false,
            debouncing: false,
            _subscription: subscription,
            tasks: Vec::new(),
        };

        store.subscribe_remote(cx);
        store.refresh(cx);
        store
    }

    /// Scope the list to an author (or clear the scope with `None`).
    pub fn set_author(&mut self, author: Option<PublicKey>, cx: &mut Context<Self>) {
        self.author = author;
        self.subscribe_remote(cx);
        self.refresh(cx);
    }

    /// Negentropy-sync announcements with the bootstrap relays.
    fn subscribe_remote(&mut self, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let author = self.author;

        backend.update(cx, |backend, cx| {
            let filter = match author {
                Some(a) => filters::announcements_by(a),
                None => filters::all_announcements(),
            };
            backend.sync_bootstrap(filter, cx);
            // Deletion requests (NIP-09/62) must be known before any
            // announcement can be shown.
            backend.sync_bootstrap(filters::deletions(), cx);
        });
    }

    /// Re-query the local database. Latest announcement per repository wins.
    ///
    /// Debounced: a short delay collapses bursts of requests (e.g. sync
    /// progress ticks), and requests that arrive while a query is running
    /// are folded into one follow-up query. The query and processing run on
    /// a background thread; only the results are applied on the main thread.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            self.refresh_dirty = true;
            return;
        }
        if self.debouncing {
            return;
        }
        self.debouncing = true;

        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH_DEBOUNCE).await;

            this.update(cx, |this, cx| {
                this.debouncing = false;
                this.run_refresh(cx);
            })
        });

        self.tasks.push(task);
    }

    /// One query + apply cycle (debounced entry point).
    fn run_refresh(&mut self, cx: &mut Context<Self>) {
        self.refreshing = true;

        let client = Backend::global(cx).read(cx).client();
        let author = self.author;

        let work = cx.background_spawn(async move {
            let filter = match author {
                Some(a) => filters::announcements_by(a),
                None => filters::all_announcements(),
            };

            let events = client.database().query(filter).await?;
            let deletion_events = client.database().query(filters::deletions()).await?;
            let deletions = Deletions::from_events(deletion_events);

            // Dedup and sort off the main thread; only the final list
            // crosses back into the entity.
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

            // Last activity per repository: state updates plus all NIP-34
            // activity events (patches, PRs, issues, statuses).
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

            // Bound the activity query to a recent window; older repos fall
            // back to their announcement / state timestamps.
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
                    // Skip events for repos we don't list, so the map can't
                    // grow beyond the number of announcements.
                    let Some(entry) = last_activity.get_mut(&addr) else {
                        continue;
                    };
                    *entry = (*entry).max(event.created_at);
                }
            }

            Ok::<_, Error>((announcements, last_activity))
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            let (announcements, last_activity) = match work.await {
                Ok(results) => results,
                // Database errors are transient; keep the last list.
                Err(_) => {
                    return this.update(cx, |this, _cx| {
                        this.refreshing = false;
                    });
                }
            };

            let again = this.update(cx, |this, cx| {
                this.announcements = Arc::new(announcements);
                this.last_activity = Arc::new(last_activity);
                cx.notify();

                this.refreshing = false;
                if this.refresh_dirty {
                    this.refresh_dirty = false;
                    true
                } else {
                    false
                }
            })?;

            // Requests that arrived while the refresh was running are
            // coalesced into one follow-up refresh.
            if again {
                this.update(cx, |this, cx| this.refresh(cx))?;
            }

            Ok(())
        }));
    }
}
