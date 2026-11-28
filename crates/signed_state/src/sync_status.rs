use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Error;
use flume::Sender;
use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use signed_core::{Deletions, Filters, RepoAddr};
use signed_git::{Repo, RepoSyncStatus};

use crate::backend::{Backend, BackendEvent};
use crate::checkouts::CheckoutsStore;
use crate::state::SyncState;

const SYNC_POLL: Duration = Duration::from_secs(180);
const MAX_SYNC_CHECKOUTS: usize = 8;

/// Statuses for these repositories may have changed; empty on signer change.
#[derive(Debug, Clone)]
pub enum SyncEvent {
    Updated(Vec<(RepoAddr, Vec<CheckoutSyncStatus>)>),
}

struct GlobalSyncStatusStore(Entity<SyncStatusStore>);

impl Global for GlobalSyncStatusStore {}

/// A checkout compared against the repository state published on Nostr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutSyncStatus {
    pub path: PathBuf,
    pub status: RepoSyncStatus,
}

/// Probes tracked repositories and broadcasts results. Holds no statuses.
pub struct SyncStatusStore {
    // Addresses a live directory still renders, with one count per directory.
    active: HashMap<RepoAddr, usize>,
    // One-shot probes that should not keep the repository on the poll.
    requested: HashSet<RepoAddr>,
    in_flight: bool,
    pass_pending: bool,
    poll_pending: bool,
    sender: Sender<()>,
    _wake_task: Task<Result<(), Error>>,
    pass_task: Option<Task<Result<(), Error>>>,
    poll_task: Option<Task<Result<(), Error>>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SyncEvent> for SyncStatusStore {}

impl SyncStatusStore {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalSyncStatusStore>().0.clone()
    }

    pub(crate) fn set_global(entity: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalSyncStatusStore(entity));
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        let (sender, receiver) = flume::unbounded::<()>();
        let mut subscriptions = Vec::new();

        if !cfg!(target_arch = "wasm32") {
            let backend = Backend::global(cx);
            let checkouts = CheckoutsStore::global(cx);

            // Checkout associations decide which paths get compared.
            subscriptions.push(cx.observe(&checkouts, |this, _checkouts, cx| {
                this.run_pass(cx);
            }));

            subscriptions.push(
                cx.subscribe(&backend, |this, _backend, event, cx| match event {
                    BackendEvent::SignerChanged => this.clear(cx),
                    BackendEvent::RepoUpdates(_) | BackendEvent::Synced => this.run_pass(cx),
                    _ => {}
                }),
            );
        }

        let task = cx.spawn(async move |this, cx| {
            loop {
                receiver.recv_async().await?;

                // One pass covers every signal queued since the wake.
                while receiver.try_recv().is_ok() {}

                this.update(cx, |this, cx| {
                    this.run_pass(cx);
                })?;
            }
        });

        Self {
            active: HashMap::new(),
            requested: HashSet::new(),
            sender,
            in_flight: false,
            pass_pending: false,
            poll_pending: false,
            _wake_task: task,
            pass_task: None,
            poll_task: None,
            _subscriptions: subscriptions,
        }
    }

    /// Requests a one-shot probe pass; folded into any pass already in flight.
    pub fn probe(&mut self, addrs: impl IntoIterator<Item = RepoAddr>) {
        self.requested.extend(addrs);
        self.wake();
    }

    /// Keeps these addresses on the periodic poll until released.
    pub fn track(&mut self, addrs: impl IntoIterator<Item = RepoAddr>) {
        for addr in addrs {
            *self.active.entry(addr).or_default() += 1;
        }
        self.wake();
    }

    /// Drops one directory's interest in these addresses.
    pub fn untrack(&mut self, addrs: impl IntoIterator<Item = RepoAddr>) {
        for addr in addrs {
            let remove = match self.active.get_mut(&addr) {
                Some(count) => {
                    *count = count.saturating_sub(1);
                    *count == 0
                }
                None => false,
            };
            if remove {
                self.active.remove(&addr);
            }
        }
    }

    fn wake(&self) {
        if let Err(e) = self.sender.send(()) {
            log::warn!("failed to queue sync status probe: {e}");
        }
    }

    // Keeps the in-flight pass so its completion releases the slot exactly once.
    fn clear(&mut self, cx: &mut Context<Self>) {
        let had_addrs = !self.active.is_empty() || !self.requested.is_empty();
        self.requested.clear();
        self.pass_pending = false;

        if had_addrs {
            cx.emit(SyncEvent::Updated(Vec::new()));
        }
    }

    fn run_pass(&mut self, cx: &mut Context<Self>) {
        if cfg!(target_arch = "wasm32") {
            return;
        }

        if self.in_flight {
            self.pass_pending = true;
            return;
        }

        let backend = Backend::global(cx);

        if backend.read(cx).current_user().is_none() {
            self.clear(cx);
            return;
        }

        if self.active.is_empty() && self.requested.is_empty() {
            return;
        }

        self.in_flight = true;
        let poll = !self.active.is_empty();
        if poll {
            self.schedule_poll(cx);
        }

        let mut targets: Vec<RepoAddr> = self.active.keys().cloned().collect();
        targets.extend(self.requested.drain());
        targets.sort();
        targets.dedup();

        let client = backend.read(cx).client();
        let targets: Vec<(RepoAddr, Vec<PathBuf>)> = {
            let checkouts = CheckoutsStore::global(cx);

            targets
                .into_iter()
                .map(|addr| {
                    let paths = checkouts.read(cx).associations_of(&addr);
                    (addr, paths)
                })
                .collect()
        };

        let work = cx.background_spawn(async move {
            let db = client.database();
            let deletion_events = db.query(Filters::deletions()).await?;
            let deletions = Deletions::from_events(deletion_events);

            let mut updates: Vec<(RepoAddr, Vec<CheckoutSyncStatus>)> = Vec::new();

            for (addr, paths) in &targets {
                let events = db.query(addr.state_filter()).await?;
                let latest = utils::latest(
                    events
                        .into_iter()
                        .filter(|event| !deletions.is_deleted(event)),
                );

                let Some(event) = latest else {
                    updates.push((addr.clone(), Vec::new()));
                    continue;
                };

                let remote_refs: Vec<(String, String)> = SyncState::parse(&event)
                    .refs
                    .iter()
                    .map(|git_ref| (git_ref.name.clone(), git_ref.commit.to_string()))
                    .collect();

                let mut list = Vec::new();
                for path in paths.iter().take(MAX_SYNC_CHECKOUTS) {
                    let synced =
                        Repo::try_open(path).and_then(|repo| repo.sync_status(&remote_refs).ok());

                    if let Some(status) = synced {
                        list.push(CheckoutSyncStatus {
                            path: path.clone(),
                            status,
                        });
                    }
                }

                updates.push((addr.clone(), list));
            }

            Ok::<_, Error>(updates)
        });

        self.pass_task = Some(cx.spawn(async move |this, cx| {
            // Reset the slot even on failure, or every later pass folds forever.
            let updates = match work.await {
                Ok(updates) => updates,
                Err(error) => {
                    log::warn!("failed to compute nostr sync statuses: {error}");
                    Vec::new()
                }
            };

            let mut again = false;

            this.update(cx, |this, cx| {
                this.in_flight = false;
                if !updates.is_empty() {
                    cx.emit(SyncEvent::Updated(updates));
                }
                again = std::mem::take(&mut this.pass_pending);
            })?;

            if again {
                this.update(cx, |this, cx| {
                    this.run_pass(cx);
                })?;
            }

            Ok::<(), anyhow::Error>(())
        }));
    }

    fn schedule_poll(&mut self, cx: &mut Context<Self>) {
        if self.poll_pending {
            return;
        }
        self.poll_pending = true;

        self.poll_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SYNC_POLL).await;

            this.update(cx, |this, cx| {
                this.poll_pending = false;
                this.run_pass(cx);
            })?;

            Ok::<(), anyhow::Error>(())
        }));
    }
}

/// Sync status cache bound to one panel or screen.
pub struct SyncStatusDirectory {
    statuses: HashMap<RepoAddr, Vec<CheckoutSyncStatus>>,
    pending: HashSet<RepoAddr>,
    tracked: HashSet<RepoAddr>,
    _subscriptions: Vec<Subscription>,
}

impl SyncStatusDirectory {
    /// Create a directory, following the sync store's events.
    pub fn new(cx: &mut App) -> Entity<Self> {
        let store = SyncStatusStore::global(cx);

        cx.new(|cx| {
            let subscription =
                cx.subscribe(&store, |this: &mut Self, _store, event, cx| match event {
                    SyncEvent::Updated(updates) => this.apply(updates.clone(), cx),
                });

            let release = cx.on_release(|this: &mut Self, cx| {
                let tracked = std::mem::take(&mut this.tracked);
                if tracked.is_empty() {
                    return;
                }
                let store = SyncStatusStore::global(cx);
                store.update(cx, |store, _| store.untrack(tracked));
            });

            Self {
                statuses: HashMap::new(),
                pending: HashSet::new(),
                tracked: HashSet::new(),
                _subscriptions: vec![subscription, release],
            }
        })
    }

    /// Queue a probe for addrs this surface renders.
    pub fn track(&mut self, addrs: impl IntoIterator<Item = RepoAddr>, cx: &mut Context<Self>) {
        let missing: Vec<RepoAddr> = addrs
            .into_iter()
            .filter(|addr| {
                !self.statuses.contains_key(addr)
                    && !self.pending.contains(addr)
                    && !self.tracked.contains(addr)
            })
            .collect();

        if missing.is_empty() {
            return;
        }

        self.pending.extend(missing.iter().cloned());
        self.tracked.extend(missing.iter().cloned());

        let store = SyncStatusStore::global(cx);
        store.update(cx, |store, _| {
            store.track(missing);
        });
    }

    /// Statuses for a repo, or empty.
    pub fn statuses_of(&self, addr: &RepoAddr) -> &[CheckoutSyncStatus] {
        self.statuses.get(addr).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Commit ids missing on Nostr, mapped to their checkout; restricted to `checkout` if tracked.
    pub fn ahead_commits_of(
        &self,
        addr: &RepoAddr,
        checkout: Option<&Path>,
    ) -> HashMap<String, PathBuf> {
        let Some(statuses) = self.statuses.get(addr) else {
            return HashMap::new();
        };

        let selected: Vec<&CheckoutSyncStatus> = match checkout
            .and_then(|path| statuses.iter().find(|status| status.path.as_path() == path))
        {
            Some(status) => vec![status],
            None => statuses.iter().collect(),
        };

        let mut commits = HashMap::new();

        for status in selected {
            if status.status.head.is_none() {
                continue;
            }
            for id in &status.status.ahead_commits {
                commits
                    .entry(id.clone())
                    .or_insert_with(|| status.path.clone());
            }
        }

        commits
    }

    fn apply(&mut self, updates: Vec<(RepoAddr, Vec<CheckoutSyncStatus>)>, cx: &mut Context<Self>) {
        if updates.is_empty() {
            let changed = !self.statuses.is_empty();
            self.statuses.clear();
            self.pending.clear();

            if changed {
                cx.notify();
            }

            return;
        }

        let mut changed = false;

        for (addr, list) in updates {
            let relevant = self.statuses.contains_key(&addr)
                || self.pending.remove(&addr)
                || self.tracked.contains(&addr);

            if !relevant {
                continue;
            }

            if self.statuses.get(&addr) != Some(&list) {
                self.statuses.insert(addr, list);
                changed = true;
            }
        }

        if changed {
            cx.notify();
        }
    }
}
