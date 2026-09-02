use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use anyhow::Error;
use flume::{Receiver, RecvTimeoutError, Sender};
use gpui::{App, AppContext, Context, Entity, Global, SharedString, Subscription, Task};
use nostr_sdk::prelude::*;
use utils::shorten_pubkey;

use crate::backend::{Backend, BackendEvent, sync_bootstrap_only};

/// A user profile (kind `0` metadata), as plain data for the UI.
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

    /// Display name, falling back to `name`, then a shortened npub.
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

        SharedString::from(shorten_pubkey(self.public_key, 4))
    }

    /// Avatar URL, if set.
    pub fn picture(&self) -> Option<SharedString> {
        self.metadata
            .picture
            .as_ref()
            .filter(|p| !p.is_empty())
            .map(|p| SharedString::from(p.clone()))
    }
}

/// Message from the fetch task to the main thread.
enum Dispatch {
    /// A batched sync finished; re-read seen profiles from the database.
    Synced,
}

/// How long to wait for more requests before firing a batched sync.
const BATCH_TIMEOUT: Duration = Duration::from_millis(500);

/// Global profile cache. Profiles are fetched in batches and kept as plain
/// data; the whole store notifies on change.
pub struct ProfileStore {
    profiles: HashMap<PublicKey, Profile>,
    /// Public keys we've already requested this session (main thread only).
    seen: RefCell<HashSet<PublicKey>>,
    /// Sender for queuing fetch requests, batched by a background task.
    sender: Sender<PublicKey>,
    tasks: Vec<Task<Result<(), Error>>>,
    _subscription: Subscription,
}

struct GlobalProfileStore(Entity<ProfileStore>);

impl Global for GlobalProfileStore {}

impl ProfileStore {
    /// Retrieve the global profile store.
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalProfileStore>().0.clone()
    }

    pub(crate) fn set_global(entity: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalProfileStore(entity));
    }

    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let backend = Backend::global(cx);

        let subscription = cx.subscribe(&backend, |this, _backend, event, cx| match event {
            BackendEvent::NostrUpdate(update) if update.kind == Kind::Metadata => {
                this.apply_author(update.author, cx);
            }
            BackendEvent::Published(event) if event.kind == Kind::Metadata => {
                let metadata = Metadata::from_json(&event.content).unwrap_or_default();
                this.profiles
                    .insert(event.pubkey, Profile::new(event.pubkey, metadata));
                cx.notify();
            }
            _ => {}
        });

        // Fetch requests are queued on a channel and synced in batches by a
        // background task.
        let client = backend.read(cx).client();
        let (sender, receiver) = flume::unbounded::<PublicKey>();
        let (dispatch_tx, dispatch_rx) = flume::unbounded::<Dispatch>();

        let mut tasks = Vec::new();

        tasks.push(cx.background_spawn(async move {
            Self::handle_requests(&client, &dispatch_tx, &receiver).await
        }));

        // Re-read seen profiles from the database after each batch sync.
        tasks.push(cx.spawn(async move |this, cx| {
            while let Ok(Dispatch::Synced) = dispatch_rx.recv_async().await {
                this.update(cx, |this, cx| this.apply_seen(cx)).ok();
            }
            Ok(())
        }));

        let mut store = Self {
            profiles: HashMap::new(),
            seen: RefCell::new(HashSet::new()),
            sender,
            tasks,
            _subscription: subscription,
        };

        store.load(cx);
        store
    }

    /// Get a profile. Returns a placeholder (default metadata) and queues a
    /// fetch if the profile isn't cached yet.
    pub fn get(&self, public_key: &PublicKey) -> Profile {
        if let Some(profile) = self.profiles.get(public_key) {
            return profile.clone();
        }

        let public_key = *public_key;

        if self.seen.borrow_mut().insert(public_key)
            && let Err(e) = self.sender.send(public_key)
        {
            log::warn!("failed to queue profile fetch: {e}");
        }

        Profile::new(public_key, Metadata::default())
    }

    /// Load recently seen profiles from the local database.
    fn load(&mut self, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let client = backend.read(cx).client();

        let work = cx.background_spawn(async move {
            let filter = Filter::new().kind(Kind::Metadata).limit(200);
            let events = client.database().query(filter).await?;

            // Parse off the main thread; only plain profiles cross back.
            let profiles: Vec<Profile> = events
                .into_iter()
                .map(|event| {
                    let metadata = Metadata::from_json(&event.content).unwrap_or_default();
                    Profile::new(event.pubkey, metadata)
                })
                .collect();

            Ok::<_, Error>(profiles)
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            let profiles = work.await?;

            this.update(cx, |this, cx| {
                for profile in profiles {
                    this.profiles.insert(profile.public_key(), profile);
                }
                cx.notify();
            })?;

            Ok(())
        }));
    }

    /// Re-read the latest metadata of an author from the local database.
    fn apply_author(&mut self, public_key: PublicKey, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let client = backend.read(cx).client();

        let work = cx.background_spawn(async move {
            let filter = Filter::new().kind(Kind::Metadata).author(public_key);
            let events = client.database().query(filter).await?;

            // Parse off the main thread; only the profile crosses back.
            let profile = events
                .into_iter()
                .max_by_key(|e| e.created_at)
                .map(|event| {
                    let metadata = Metadata::from_json(event.content).unwrap_or_default();
                    Profile::new(event.pubkey, metadata)
                });

            Ok::<_, Error>(profile)
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            let profile = work.await?;

            this.update(cx, |this, cx| {
                if let Some(profile) = profile {
                    this.profiles.insert(profile.public_key(), profile);
                    cx.notify();
                }
            })?;

            Ok(())
        }));
    }

    /// Re-read the latest metadata of every requested author from the local
    /// database (used after a sync, which produces no NostrUpdate events).
    fn apply_seen(&mut self, cx: &mut Context<Self>) {
        let authors: Vec<PublicKey> = self.seen.borrow().iter().copied().collect();

        if authors.is_empty() {
            return;
        }

        let backend = Backend::global(cx);
        let client = backend.read(cx).client();

        let work = cx.background_spawn(async move {
            let filter = Filter::new().kind(Kind::Metadata).authors(authors);
            let events = client.database().query(filter).await?;

            // Pick the latest metadata per author off the main thread.
            let mut latest: HashMap<PublicKey, (Timestamp, Metadata)> = HashMap::new();
            for event in events {
                match latest.get(&event.pubkey) {
                    Some((ts, _)) if *ts >= event.created_at => {}
                    _ => {
                        latest.insert(
                            event.pubkey,
                            (
                                event.created_at,
                                Metadata::from_json(&event.content).unwrap_or_default(),
                            ),
                        );
                    }
                }
            }

            let profiles: Vec<Profile> = latest
                .into_iter()
                .map(|(public_key, (_, metadata))| Profile::new(public_key, metadata))
                .collect();

            Ok::<_, Error>(profiles)
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            let profiles = work.await?;

            this.update(cx, |this, cx| {
                for profile in profiles {
                    this.profiles.insert(profile.public_key(), profile);
                }
                cx.notify();
            })?;

            Ok(())
        }));
    }

    /// Sync metadata for requested authors in batches, debounced to collect
    /// requests. Runs on a background thread; results are dispatched to the
    /// main thread, which re-reads the database.
    async fn handle_requests(
        client: &Client,
        dispatch: &Sender<Dispatch>,
        receiver: &Receiver<PublicKey>,
    ) -> Result<(), Error> {
        let mut batch: HashSet<PublicKey> = HashSet::new();

        loop {
            // Wait for the first request of a batch.
            match receiver.recv_timeout(BATCH_TIMEOUT) {
                Ok(public_key) => {
                    batch.insert(public_key);
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
                Err(RecvTimeoutError::Timeout) => continue,
            };

            // Collect everything that arrives within the debounce window.
            let deadline = Instant::now() + BATCH_TIMEOUT;
            while let Ok(public_key) = receiver.recv_deadline(deadline) {
                batch.insert(public_key);
            }

            let filter = Filter::new()
                .kind(Kind::Metadata)
                .authors(batch.drain().collect::<Vec<PublicKey>>());

            // Negentropy-sync with the bootstrap relays. Synced events are
            // written to the database directly (no NostrUpdate), so re-apply
            // from the database afterwards.
            match sync_bootstrap_only(client, filter, SyncOptions::default()).await {
                Ok(_) => {
                    if dispatch.send(Dispatch::Synced).is_err() {
                        log::warn!("profile dispatch channel closed, dropping sync result");
                    }
                }
                Err(e) => log::warn!("profile sync failed: {e}"),
            }
        }
    }
}
