use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::Error;
use gpui::{App, Context, Entity, Global, SharedString, Subscription, Task};
use nostr_sdk::prelude::*;

use crate::backend::{Backend, BackendEvent};

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

/// Shorten a [`PublicKey`] to `npub1abc...wxyz` form.
pub fn shorten_pubkey(public_key: PublicKey, len: usize) -> String {
    let npub = public_key.to_bech32().unwrap();
    format!("{}...{}", &npub[..(len + 5)], &npub[npub.len() - len..])
}

/// Global profile cache. Profiles are fetched in batches and kept as plain
/// data; the whole store notifies on change.
pub struct ProfileStore {
    profiles: HashMap<PublicKey, Profile>,
    /// Public keys we've already requested this session.
    seen: HashSet<PublicKey>,
    /// Public keys queued for the next batched fetch.
    queued: HashSet<PublicKey>,
    fetching: bool,
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

        let mut store = Self {
            profiles: HashMap::new(),
            seen: HashSet::new(),
            queued: HashSet::new(),
            fetching: false,
            tasks: Vec::new(),
            _subscription: subscription,
        };

        store.load(cx);
        store
    }

    /// Get a profile. Returns a placeholder (default metadata) and queues a
    /// fetch if the profile isn't cached yet.
    pub fn get(&mut self, public_key: PublicKey, cx: &mut Context<Self>) -> Profile {
        if let Some(profile) = self.profiles.get(&public_key) {
            return profile.clone();
        }

        if self.seen.insert(public_key) {
            self.queued.insert(public_key);
            self.queue_fetch(cx);
        }

        Profile::new(public_key, Metadata::default())
    }

    /// Load recently seen profiles from the local database.
    fn load(&mut self, cx: &mut Context<Self>) {
        let client = Backend::global(cx).read(cx).client();

        let task = cx.spawn(async move |this, cx| {
            let filter = Filter::new().kind(Kind::Metadata).limit(200);
            let events = client.database().query(filter).await?;

            this.update(cx, |this, cx| {
                for event in events {
                    let metadata = Metadata::from_json(&event.content).unwrap_or_default();
                    this.profiles
                        .insert(event.pubkey, Profile::new(event.pubkey, metadata));
                }
                cx.notify();
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    /// Re-read the latest metadata of an author from the local database.
    fn apply_author(&mut self, public_key: PublicKey, cx: &mut Context<Self>) {
        let client = Backend::global(cx).read(cx).client();

        let task = cx.spawn(async move |this, cx| {
            let filter = Filter::new().kind(Kind::Metadata).author(public_key);
            let events = client.database().query(filter).await?;

            if let Some(event) = events.into_iter().max_by_key(|e| e.created_at) {
                let metadata = Metadata::from_json(event.content).unwrap_or_default();

                this.update(cx, |this, cx| {
                    this.profiles
                        .insert(public_key, Profile::new(public_key, metadata));
                    cx.notify();
                })?;
            }

            Ok(())
        });

        self.tasks.push(task);
    }

    /// Drain the queue in a batched fetch, debounced to collect requests.
    fn queue_fetch(&mut self, cx: &mut Context<Self>) {
        if self.fetching {
            return;
        }
        self.fetching = true;

        let client = Backend::global(cx).read(cx).client();

        let task = cx.spawn(async move |this, cx| {
            loop {
                // Collect more requests before firing the batch.
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;

                let batch = this.update(cx, |this, _cx| std::mem::take(&mut this.queued))?;

                if batch.is_empty() {
                    this.update(cx, |this, _cx| {
                        this.fetching = false;
                    })?;
                    break;
                }

                let filter = Filter::new()
                    .kind(Kind::Metadata)
                    .authors(batch.into_iter().collect::<Vec<PublicKey>>());

                // Gossip routes the fetch to each author's relays. Fetched
                // events land in the database and surface via NostrUpdate.
                if let Err(e) = client.fetch_events(filter).await {
                    log::warn!("profile fetch failed: {e}");
                }
            }

            Ok(())
        });

        self.tasks.push(task);
    }
}
