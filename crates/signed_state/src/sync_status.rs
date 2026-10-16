use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
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

struct GlobalSyncStatusStore(Entity<SyncStatusStore>);

impl Global for GlobalSyncStatusStore {}

/// A checkout compared against the repository state published on Nostr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutSyncStatus {
    pub path: PathBuf,
    pub status: RepoSyncStatus,
}

/// Statuses for these repositories may have changed; empty on signer change.
#[derive(Debug, Clone)]
pub enum SyncEvent {
    Updated(Vec<(RepoAddr, Vec<CheckoutSyncStatus>)>),
}

/// Probes tracked repositories and broadcasts results. Holds no statuses.
pub struct SyncStatusStore {
    active: RefCell<HashSet<RepoAddr>>,
    in_flight: bool,
    pass_pending: bool,
    poll_pending: bool,
    sender: Sender<()>,
    tasks: Vec<Task<Result<(), Error>>>,
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
            active: RefCell::new(HashSet::new()),
            sender,
            in_flight: false,
            pass_pending: false,
            poll_pending: false,
            tasks: vec![task],
            _subscriptions: subscriptions,
        }
    }

    /// Queue a probe pass; folded into any pass already in flight.
    pub fn probe(&self, addrs: impl IntoIterator<Item = RepoAddr>) {
        self.active.borrow_mut().extend(addrs);

        if let Err(e) = self.sender.send(()) {
            log::warn!("failed to queue sync status probe: {e}");
        }
    }

    // Keeps the in-flight pass so its completion releases the slot exactly once.
    fn clear(&mut self, cx: &mut Context<Self>) {
        let had_addrs = !self.active.borrow().is_empty();
        self.active.borrow_mut().clear();
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
        let client = backend.read(cx).client();

        if backend.read(cx).current_user().is_none() {
            self.clear(cx);
            return;
        }

        if self.active.borrow().is_empty() {
            return;
        }

        self.in_flight = true;
        self.schedule_poll(cx);

        let targets: Vec<(RepoAddr, Vec<PathBuf>)> = {
            let active = self.active.borrow();
            let checkouts = CheckoutsStore::global(cx);

            active
                .iter()
                .map(|addr| {
                    let paths = checkouts.read(cx).associations_of(addr);
                    (addr.clone(), paths)
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

        self.tasks.push(cx.spawn(async move |this, cx| {
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

        self.tasks.push(cx.spawn(async move |this, cx| {
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
    pending: RefCell<HashSet<RepoAddr>>,
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

            Self {
                statuses: HashMap::new(),
                pending: RefCell::new(HashSet::new()),
                _subscriptions: vec![subscription],
            }
        })
    }

    /// Queue a probe for addrs this surface renders.
    pub fn track(&mut self, addrs: impl IntoIterator<Item = RepoAddr>, cx: &mut Context<Self>) {
        let missing: Vec<RepoAddr> = addrs
            .into_iter()
            .filter(|addr| {
                !self.statuses.contains_key(addr)
                    && !self.with_pending(|pending| pending.contains(addr))
            })
            .collect();

        if missing.is_empty() {
            return;
        }

        self.with_pending(|pending| pending.extend(missing.iter().cloned()));

        let store = SyncStatusStore::global(cx);
        store.read(cx).probe(missing);
    }

    /// Statuses for a repo, or empty; a miss queues a probe.
    pub fn statuses_of(&self, addr: &RepoAddr, cx: &App) -> Vec<CheckoutSyncStatus> {
        self.queue(addr, cx);
        self.statuses.get(addr).cloned().unwrap_or_default()
    }

    /// Commits available locally but missing on Nostr.
    pub fn unsynced(&self, addr: &RepoAddr, cx: &App) -> usize {
        self.queue(addr, cx);

        self.statuses
            .get(addr)
            .map(|list| {
                list.iter()
                    .map(|checkout| checkout.status.ahead_total)
                    .sum()
            })
            .unwrap_or(0)
    }

    fn queue(&self, addr: &RepoAddr, cx: &App) {
        if self.with_pending(|pending| pending.insert(addr.clone())) {
            let store = SyncStatusStore::global(cx);
            store.read(cx).probe([addr.clone()]);
        }
    }

    fn apply(&mut self, updates: Vec<(RepoAddr, Vec<CheckoutSyncStatus>)>, cx: &mut Context<Self>) {
        if updates.is_empty() {
            let changed = !self.statuses.is_empty();
            self.statuses.clear();
            self.with_pending(|pending| pending.clear());

            if changed {
                cx.notify();
            }

            return;
        }

        let mut changed = false;

        for (addr, list) in updates {
            let relevant = self.statuses.contains_key(&addr)
                || self.with_pending(|pending| pending.remove(&addr));

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

    fn with_pending<R>(&self, then: impl FnOnce(&mut HashSet<RepoAddr>) -> R) -> R {
        let mut pending = self.pending.borrow_mut();
        then(&mut pending)
    }
}
