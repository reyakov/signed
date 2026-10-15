use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use flume::Sender;
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, Global, SharedString, Subscription, Task,
};
use nostr_sdk::prelude::*;
use signed_state::{Backend, BackendEvent, sync_bootstrap_only};
use utils::shorten_pubkey;

const BATCH_TIMEOUT: Duration = Duration::from_millis(500);
const REQUEST_CHUNK: usize = 100;

#[derive(Debug, Clone)]
pub struct Profile {
    public_key: PublicKey,
    metadata: Metadata,
}

impl Profile {
    pub fn new(public_key: PublicKey, metadata: Metadata) -> Self {
        Self {
            public_key,
            metadata,
        }
    }

    pub fn public_key(&self) -> PublicKey {
        self.public_key
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    // Falls back to `name`, then a shortened npub.
    pub fn name(&self) -> SharedString {
        if let Some(display_name) = self.metadata.display_name.as_ref()
            && !display_name.is_empty()
        {
            return SharedString::from(display_name.trim().to_owned());
        }

        if let Some(name) = self.metadata.name.as_ref()
            && !name.is_empty()
        {
            return SharedString::from(name.trim().to_owned());
        }

        SharedString::from(shorten_pubkey(self.public_key))
    }

    pub fn picture(&self) -> Option<SharedString> {
        self.metadata
            .picture
            .as_ref()
            .filter(|p| !p.is_empty())
            .map(|p| SharedString::from(p.clone()))
    }

    /// The newest profile per author from metadata events.
    fn latest_profiles(events: impl IntoIterator<Item = Event>) -> Vec<Self> {
        let mut latest: HashMap<PublicKey, (Timestamp, Metadata)> = HashMap::new();

        for event in events {
            match latest.get(&event.pubkey) {
                Some((created_at, _)) if *created_at >= event.created_at => {}
                _ => {
                    let metadata = Metadata::from_json(&event.content).unwrap_or_default();
                    latest.insert(event.pubkey, (event.created_at, metadata));
                }
            }
        }

        latest
            .into_iter()
            .map(|(public_key, (_, metadata))| Profile::new(public_key, metadata))
            .collect()
    }
}

/// Metadata for these authors may have changed in the local database.
#[derive(Debug, Clone)]
pub enum ProfileEvent {
    Updated(Vec<PublicKey>),
}

/// Batches metadata requests into relay syncs feeding the local database.
pub struct ProfileStore {
    in_flight: RefCell<HashSet<PublicKey>>,
    sender: Sender<PublicKey>,
    _task: Task<Result<(), anyhow::Error>>,
    _subscription: Subscription,
}

struct GlobalProfileStore(Entity<ProfileStore>);

impl Global for GlobalProfileStore {}

impl EventEmitter<ProfileEvent> for ProfileStore {}

impl ProfileStore {
    pub(crate) fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalProfileStore>().0.clone()
    }

    pub(crate) fn set_global(entity: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalProfileStore(entity));
    }

    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let backend = Backend::global(cx);
        let client = backend.read(cx).client();

        let (sender, receiver) = flume::unbounded::<PublicKey>();

        let subscription = cx.subscribe(&backend, |this, _backend, event, cx| {
            if let BackendEvent::ProfileUpdates(authors) = event {
                this.emit_updated(authors.clone(), cx);
            }
        });

        let task = cx.spawn(async move |this, cx| {
            let mut batch: HashSet<PublicKey> = HashSet::new();

            loop {
                match receiver.recv_async().await {
                    Ok(public_key) => {
                        batch.insert(public_key);
                    }
                    Err(_) => return Ok(()),
                }

                // The channel has no async timeout, race the receive against a timer.
                let deadline = Instant::now() + BATCH_TIMEOUT;

                loop {
                    let now = Instant::now();

                    if now >= deadline {
                        break;
                    }

                    let timer = cx.background_executor().timer(deadline - now);
                    futures::pin_mut!(timer);

                    let recv = receiver.recv_async();
                    futures::pin_mut!(recv);

                    match futures::future::select(recv, timer).await {
                        futures::future::Either::Left((Ok(public_key), _)) => {
                            batch.insert(public_key);
                        }
                        futures::future::Either::Left((Err(_), _)) => return Ok(()),
                        futures::future::Either::Right(_) => break,
                    }
                }

                let authors: Vec<PublicKey> = batch.drain().collect();

                for chunk in authors.chunks(REQUEST_CHUNK) {
                    let opts = SyncOptions::default();
                    let filter = Filter::new()
                        .kind(Kind::Metadata)
                        .authors(chunk.iter().copied());

                    if let Err(e) = sync_bootstrap_only(&client, filter, opts).await {
                        log::warn!("profile fetch failed: {e}");
                    }
                }

                // Emit even when the sync failed, the database may hold newer data.
                this.update(cx, |this, cx| {
                    this.emit_updated(authors, cx);
                })?;
            }
        });

        Self {
            in_flight: RefCell::new(HashSet::new()),
            sender,
            _task: task,
            _subscription: subscription,
        }
    }

    /// Queue a batched relay sync for keys without one in flight.
    pub fn fetch(&self, keys: impl IntoIterator<Item = PublicKey>) {
        let mut queue = Vec::new();
        {
            let mut in_flight = self.in_flight.borrow_mut();

            for key in keys {
                if in_flight.insert(key) {
                    queue.push(key);
                }
            }
        }
        for public_key in queue {
            if let Err(e) = self.sender.send(public_key) {
                log::warn!("failed to queue profile fetch: {e}");
            }
        }
    }

    // Releases the in-flight keys so future fetches can be queued again.
    fn emit_updated(&mut self, authors: Vec<PublicKey>, cx: &mut Context<Self>) {
        if authors.is_empty() {
            return;
        }

        {
            let mut in_flight = self.in_flight.borrow_mut();
            for author in &authors {
                in_flight.remove(author);
            }
        }

        cx.emit(ProfileEvent::Updated(authors));
    }
}

/// Profile cache bound to one panel or screen.
pub struct ProfileDirectory {
    profiles: HashMap<PublicKey, Profile>,
    pending: RefCell<HashSet<PublicKey>>,
    tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ProfileDirectory {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let store = ProfileStore::global(cx);

        cx.new(|cx| {
            let subscription =
                cx.subscribe(&store, |this: &mut Self, _store, event, cx| match event {
                    ProfileEvent::Updated(authors) => this.refresh(authors, cx),
                });

            Self {
                profiles: HashMap::new(),
                pending: RefCell::new(HashSet::new()),
                tasks: Vec::new(),
                _subscriptions: vec![subscription],
            }
        })
    }

    /// Resolve keys from the local database, queue the rest for a relay sync.
    pub fn track<K>(&mut self, keys: K, cx: &mut Context<Self>)
    where
        K: IntoIterator<Item = PublicKey>,
    {
        let missing: Vec<PublicKey> = keys
            .into_iter()
            .filter(|key| {
                !self.profiles.contains_key(key)
                    && !self.with_pending(|pending| pending.contains(key))
            })
            .collect();

        if missing.is_empty() {
            return;
        }

        self.load(missing, true, cx);
    }

    /// The profile for a key, or a placeholder that queues a relay sync.
    pub fn profile(&self, public_key: &PublicKey, cx: &App) -> Profile {
        if let Some(profile) = self.profiles.get(public_key) {
            return profile.clone();
        }

        if self.with_pending(|pending| pending.insert(*public_key)) {
            let store = ProfileStore::global(cx);
            store.read(cx).fetch([*public_key]);
        }

        Profile::new(*public_key, Metadata::default())
    }

    /// Refreshes profiles for keys that are already tracked.
    fn refresh(&mut self, authors: &[PublicKey], cx: &mut Context<Self>) {
        let tracked: Vec<PublicKey> = authors
            .iter()
            .copied()
            .filter(|key| {
                self.profiles.contains_key(key)
                    || self.with_pending(|pending| pending.contains(key))
            })
            .collect();

        if tracked.is_empty() {
            return;
        }

        self.load(tracked, false, cx);
    }

    /// Loads profiles from the local database, queueing a relay sync for any missing keys.
    fn load(&mut self, keys: Vec<PublicKey>, queue_sync: bool, cx: &mut Context<Self>) {
        // Accept events that arrive while the database query is running.
        self.with_pending(|pending| {
            pending.extend(keys.iter().copied());
        });

        let backend = Backend::global(cx);
        let client = backend.read(cx).client();
        let query_keys = keys.clone();

        let work = cx.background_spawn(async move {
            let authors = query_keys.iter().copied();
            let filter = Filter::new().kind(Kind::Metadata).authors(authors);
            let events = client.database().query(filter).await?;

            Ok::<_, anyhow::Error>(Profile::latest_profiles(events))
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            let loaded = work.await;

            this.update(cx, |this, cx| {
                let remaining = match loaded {
                    Ok(profiles) => {
                        let found: HashSet<PublicKey> =
                            profiles.iter().map(Profile::public_key).collect();
                        this.insert_loaded(profiles, cx);
                        keys.into_iter()
                            .filter(|key| !found.contains(key))
                            .collect()
                    }
                    Err(e) => {
                        log::warn!("failed to load profiles from the database: {e}");
                        keys
                    }
                };

                if queue_sync && !remaining.is_empty() {
                    let store = ProfileStore::global(cx);
                    store.read(cx).fetch(remaining);
                }
            })
            .ok();
        }));
    }

    // Inserts database results, replacing anything loaded earlier.
    fn insert_loaded(&mut self, profiles: Vec<Profile>, cx: &mut Context<Self>) {
        let mut inserted = false;

        for profile in profiles {
            let public_key = profile.public_key();
            self.profiles.insert(public_key, profile);
            self.with_pending(|pending| pending.remove(&public_key));
            inserted = true;
        }

        if inserted {
            cx.notify();
        }
    }

    fn with_pending<R, T>(&self, then: T) -> R
    where
        T: FnOnce(&mut HashSet<PublicKey>) -> R,
    {
        let mut pending = self.pending.borrow_mut();
        then(&mut pending)
    }
}
