use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use anyhow::Error;
use flume::{Receiver, Sender};
use gpui::{
    App, AppContext, AsyncApp, Context, Entity, Global, SharedString, Subscription, Task,
    WeakEntity,
};
use nostr_sdk::prelude::*;
use utils::shorten_pubkey;

use crate::backend::{Backend, BackendEvent, sync_bootstrap_only};

/// A user profile as plain data for the UI, from the kind-0 metadata.
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

/// How long to wait for more requests before firing a batched sync.
const BATCH_TIMEOUT: Duration = Duration::from_millis(500);

/// Global profile cache.
///
/// Profiles are fetched in batches and kept as plain data.
pub struct ProfileStore {
    profiles: HashMap<PublicKey, Profile>,
    /// Public keys requested this session, main thread only.
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

        // Fetch requests are queued on a channel, batched into one sync per debounce window.
        let client = backend.read(cx).client();
        let (sender, receiver) = flume::unbounded::<PublicKey>();
        let entity = cx.entity().downgrade();

        let mut tasks = Vec::new();

        tasks.push(cx.spawn(async move |_this, cx| {
            Self::handle_requests(entity, &client, &receiver, cx).await
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

    /// Track a spawned task, pruning finished tasks first.
    ///
    /// Keeps the store's task list bounded by the number of in-flight tasks.
    fn push_task(&mut self, task: Task<Result<(), Error>>) {
        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(task);
    }

    /// Get a profile.
    ///
    /// Returns a placeholder with default metadata. Queues a fetch when the profile is not cached yet.
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

            // Parse off the main thread.
            // Only plain profiles cross back.
            let profiles: Vec<Profile> = events
                .into_iter()
                .map(|event| {
                    let metadata = Metadata::from_json(&event.content).unwrap_or_default();
                    Profile::new(event.pubkey, metadata)
                })
                .collect();

            Ok::<_, Error>(profiles)
        });

        self.push_task(cx.spawn(async move |this, cx| {
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

            // Parse off the main thread.
            // Only the profile crosses back.
            let profile = events
                .into_iter()
                .max_by_key(|e| e.created_at)
                .map(|event| {
                    let metadata = Metadata::from_json(event.content).unwrap_or_default();
                    Profile::new(event.pubkey, metadata)
                });

            Ok::<_, Error>(profile)
        });

        self.push_task(cx.spawn(async move |this, cx| {
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

    /// Re-read the latest metadata of every requested author from the local database.
    ///
    /// Used after a sync, which produces no NostrUpdate events.
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

        self.push_task(cx.spawn(async move |this, cx| {
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

    /// Sync metadata for requested authors in batches, debounced to collect requests.
    ///
    /// After each batch, the seen profiles are re-read from the database on the main thread.
    async fn handle_requests(
        this: WeakEntity<ProfileStore>,
        client: &Client,
        receiver: &Receiver<PublicKey>,
        cx: &mut AsyncApp,
    ) -> Result<(), Error> {
        let mut batch: HashSet<PublicKey> = HashSet::new();

        loop {
            // Wait for the first request of a batch.
            match receiver.recv_async().await {
                Ok(public_key) => {
                    batch.insert(public_key);
                }
                Err(_) => return Ok(()),
            }

            // Collect everything that arrives within the debounce window.
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

            let filter = Filter::new()
                .kind(Kind::Metadata)
                .authors(batch.drain().collect::<Vec<PublicKey>>());

            // Negentropy-sync with the bootstrap relays.
            // Synced events are written to the database directly, no NostrUpdate.
            // Re-apply from the database afterwards.
            match sync_bootstrap_only(client, filter, SyncOptions::default()).await {
                Ok(_) => {
                    let _ = this.update(cx, |this, cx| this.apply_seen(cx));
                }
                Err(e) => log::warn!("profile sync failed: {e}"),
            }
        }
    }
}
