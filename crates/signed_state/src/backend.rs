use std::time::Duration;

use anyhow::{Error, anyhow};
use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use nostr_connect::prelude::*;
use nostr_sdk::prelude::*;
use signed_core::filters;
use signed_nostr::{NostrBackend, SignedAuthUrlHandler, UniversalSigner, Update};

/// Keyring entry holding the user credential (`nsec1...` or `bunker://...`).
pub const USER_KEYRING: &str = "su.reya.signed#user";
/// Keyring entry holding the locally generated key for NIP-46 sessions.
pub const MASTER_KEYRING: &str = "su.reya.signed#master";
/// Timeout for NIP-46 signer responses.
pub const NOSTR_CONNECT_TIMEOUT: u64 = 60;

/// Relays connected at startup, before any user-specific relay config is known.
pub const BOOTSTRAP_RELAYS: [&str; 4] = [
    "wss://relay.primal.net",
    "wss://relay.ditto.pub",
    "wss://index.ngit.dev",
    "wss://profiles.nostr1.com",
];

/// Relays used for indexing user's relay list (NIP-65).
pub const INDEXER_RELAYS: [&str; 3] = [
    "wss://indexer.coracle.social",
    "wss://purplepag.es",
    "wss://user.kindpag.es",
];

#[derive(Debug, Clone)]
pub enum BackendEvent {
    /// User has no signer configured.
    SignerRequired,
    /// The signer has changed (login/logout/account switch).
    SignerChanged,
    /// Relay bootstrap finished.
    Connected,
    /// A new event was received from a relay and stored in the database.
    NostrUpdate(Update),
    /// An event built locally was signed, broadcast and stored.
    Published(Box<Event>),
    /// An error occurred.
    Error(String),
}

impl BackendEvent {
    pub fn error<T>(error: T) -> Self
    where
        T: Into<String>,
    {
        Self::Error(error.into())
    }
}

/// Global backend entity: owns the nostr client, the signer and the
/// notification pump. Stores subscribe to [`BackendEvent`] and re-query the
/// local database when relevant updates arrive.
pub struct Backend {
    inner: NostrBackend,
    current_user: Option<PublicKey>,
    tasks: Vec<Task<Result<(), Error>>>,
}

struct GlobalBackend(Entity<Backend>);

impl Global for GlobalBackend {}

impl EventEmitter<BackendEvent> for Backend {}

impl Backend {
    /// Retrieve the global backend.
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalBackend>().0.clone()
    }

    pub(crate) fn set_global(entity: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalBackend(entity));
    }

    pub(crate) fn new(inner: NostrBackend, cx: &mut Context<Self>) -> Self {
        let client = inner.client();

        let pump = cx.spawn(async move |this, cx| {
            let mut notifications = client.notifications();

            while let Some(notification) = notifications.next().await {
                let ClientNotification::Event { event, .. } = notification else {
                    continue;
                };

                let update = Update::from_event(&event);

                if this
                    .update(cx, |_, cx| cx.emit(BackendEvent::NostrUpdate(update)))
                    .is_err()
                {
                    break;
                }
            }

            Ok(())
        });

        let mut this = Self {
            inner,
            current_user: None,
            tasks: vec![pump],
        };

        this.bootstrap(cx);
        this
    }

    /// Bootstrap the client: connect to the default relays (indexers as
    /// discovery-only) and restore the saved session, if any.
    fn bootstrap(&mut self, cx: &mut Context<Self>) {
        let backend = self.inner.clone();

        let task = cx.background_spawn(async move {
            for url in BOOTSTRAP_RELAYS {
                backend.add_relay(url).await?;
            }
            for url in INDEXER_RELAYS {
                backend.add_discovery_relay(url).await?;
            }
            backend.connect().await;
            Ok::<(), Error>(())
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            match task.await {
                Ok(()) => {
                    this.update(cx, |_this, cx| cx.emit(BackendEvent::Connected))?;
                }
                Err(e) => {
                    this.update(cx, |_this, cx| cx.emit(BackendEvent::error(e.to_string())))?;
                }
            }
            Ok(())
        }));

        self.restore_session(cx);
    }

    /// Restore the saved session from the keyring. Emits
    /// [`BackendEvent::SignerRequired`] if no credential is stored.
    pub fn restore_session(&mut self, cx: &mut Context<Self>) {
        if cfg!(target_arch = "wasm32") {
            cx.emit(BackendEvent::SignerRequired);
            return;
        }

        let user = cx.read_credentials(USER_KEYRING);
        let master = self.master_key(cx);

        self.tasks.push(cx.spawn(async move |this, cx| {
            let content = match user.await {
                Ok(Some((_username, secret))) => String::from_utf8(secret)?,
                _ => {
                    this.update(cx, |_, cx| cx.emit(BackendEvent::SignerRequired))?;
                    return Ok(());
                }
            };

            let result = async {
                if content.starts_with("nsec1") {
                    let keys = Keys::new(SecretKey::parse(&content)?);
                    this.update(cx, |this, cx| this.set_signer(keys, cx))?;
                } else if content.starts_with("bunker://") {
                    let uri = NostrConnectUri::parse(&content)?;
                    let mut signer = NostrConnect::new(
                        uri,
                        master.await,
                        Duration::from_secs(NOSTR_CONNECT_TIMEOUT),
                        None,
                    )?;
                    signer.auth_url_handler(SignedAuthUrlHandler);
                    this.update(cx, |this, cx| this.set_signer(signer, cx))?;
                } else {
                    this.update(cx, |_, cx| cx.emit(BackendEvent::SignerRequired))?;
                }

                Ok::<_, Error>(())
            }
            .await;

            if let Err(e) = result {
                this.update(cx, |_, cx| {
                    cx.emit(BackendEvent::error(e.to_string()));
                    cx.emit(BackendEvent::SignerRequired);
                })?;
            }

            Ok(())
        }));
    }

    /// Login with an `nsec1...` secret key. The credential is verified by
    /// the signer flow and persisted in the keyring.
    pub fn login_with_nsec(&mut self, nsec: &str, cx: &mut Context<Self>) {
        let nsec = nsec.trim().to_owned();

        let keys = match SecretKey::parse(&nsec) {
            Ok(secret) => Keys::new(secret),
            Err(e) => {
                cx.emit(BackendEvent::error(e.to_string()));
                return;
            }
        };

        let write =
            cx.write_credentials(USER_KEYRING, &keys.public_key().to_hex(), nsec.as_bytes());

        self.tasks.push(cx.spawn(async move |this, cx| {
            if let Err(e) = write.await {
                this.update(cx, |_, cx| cx.emit(BackendEvent::error(e.to_string())))?;
                return Ok(());
            }

            this.update(cx, |this, cx| this.set_signer(keys, cx))?;
            Ok(())
        }));
    }

    /// Login with a `bunker://...` URI (NIP-46). The auth URL, if any, is
    /// opened in the default browser. The credential is persisted in the
    /// keyring after the signer proves reachable.
    pub fn login_with_bunker(&mut self, uri: &str, cx: &mut Context<Self>) {
        let uri_string = uri.trim().to_owned();

        let connect_uri = match NostrConnectUri::parse(&uri_string) {
            Ok(uri) => uri,
            Err(e) => {
                cx.emit(BackendEvent::error(e.to_string()));
                return;
            }
        };

        let master = self.master_key(cx);
        let write = cx.write_credentials(USER_KEYRING, "bunker", uri_string.as_bytes());

        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = async {
                let mut signer = NostrConnect::new(
                    connect_uri,
                    master.await,
                    Duration::from_secs(NOSTR_CONNECT_TIMEOUT),
                    None,
                )?;
                signer.auth_url_handler(SignedAuthUrlHandler);

                // Verify the signer before persisting the credential.
                signer.get_public_key_async().await?;
                write.await?;

                this.update(cx, |this, cx| this.set_signer(signer, cx))?;

                Ok::<_, Error>(())
            }
            .await;

            if let Err(e) = result {
                this.update(cx, |_, cx| cx.emit(BackendEvent::error(e.to_string())))?;
            }

            Ok(())
        }));
    }

    /// Remove the saved credential and reset to an anonymous session.
    pub fn logout(&mut self, cx: &mut Context<Self>) {
        let delete = cx.delete_credentials(USER_KEYRING);

        self.tasks.push(cx.spawn(async move |this, cx| {
            delete.await.ok();

            this.update(cx, |this, cx| {
                this.inner.signer().swap_inner(Keys::generate());
                this.current_user = None;
                cx.emit(BackendEvent::SignerChanged);
                cx.emit(BackendEvent::SignerRequired);
                cx.notify();
            })?;

            Ok(())
        }));
    }

    /// Get (or generate and persist) the key used for NIP-46 sessions.
    fn master_key(&self, cx: &App) -> Task<Keys> {
        let task = cx.read_credentials(MASTER_KEYRING);

        cx.spawn(async move |cx| {
            let (keys, new_key) = match task.await {
                Ok(Some((_user, secret))) => match SecretKey::from_slice(&secret) {
                    Ok(secret_key) => (Keys::new(secret_key), false),
                    _ => (Keys::generate(), true),
                },
                _ => (Keys::generate(), true),
            };

            if new_key {
                let username = keys.public_key().to_hex();
                let password = keys.secret_key().to_secret_bytes();

                cx.update(|cx| {
                    let task = cx.write_credentials(MASTER_KEYRING, &username, &password);
                    cx.background_spawn(async move { task.await.ok() }).detach();
                });
            }

            keys
        })
    }

    /// Fetch the user's grasp list (kind `10317`) and add the listed grasp
    /// servers as relays.
    fn bootstrap_user(&mut self, public_key: PublicKey, cx: &mut Context<Self>) {
        let backend = self.inner.clone();

        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = async {
                let events = backend
                    .client()
                    .fetch_events(filters::grasp_list(public_key))
                    .await?;

                let urls: Vec<String> = events
                    .into_iter()
                    .max_by_key(|e| e.created_at)
                    .map(|e| {
                        e.tags
                            .iter()
                            .filter(|t| t.kind() == "g")
                            .filter_map(|t| t.content().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();

                for url in urls {
                    backend.add_relay(&url).await.ok();
                }
                backend.connect().await;

                Ok::<_, Error>(())
            }
            .await;

            if let Err(e) = result {
                this.update(cx, |_, cx| cx.emit(BackendEvent::error(e.to_string())))?;
            }

            Ok(())
        }));
    }

    /// Get the nostr client.
    pub fn client(&self) -> Client {
        self.inner.client()
    }

    /// Get the current signer.
    pub fn signer(&self) -> UniversalSigner {
        self.inner.signer()
    }

    /// Get the current user's public key.
    pub fn current_user(&self) -> Option<PublicKey> {
        self.current_user
    }

    /// Update the signer (any type implementing the async signer traits,
    /// e.g. `Keys`, `NostrConnect`, a browser extension proxy).
    pub fn set_signer<T>(&mut self, new_signer: T, cx: &mut Context<Self>)
    where
        T: AsyncGetPublicKey + AsyncSignEvent + AsyncNip44 + 'static,
        <T as AsyncGetPublicKey>::Error: std::error::Error + Send + Sync + 'static,
        <T as AsyncSignEvent>::Error: std::error::Error + Send + Sync + 'static,
        <T as AsyncNip44>::Error: std::error::Error + Send + Sync + 'static,
    {
        let task = cx.spawn(async move |this, cx| {
            match new_signer.get_public_key_async().await {
                Ok(public_key) => {
                    this.update(cx, |this, cx| {
                        this.inner.signer().swap_inner(new_signer);
                        this.current_user = Some(public_key);
                        this.bootstrap_user(public_key, cx);
                        cx.emit(BackendEvent::SignerChanged);
                        cx.notify();
                    })?;
                }
                Err(e) => {
                    this.update(cx, |_this, cx| {
                        cx.emit(BackendEvent::error(e.to_string()));
                    })?;
                }
            }

            Ok(())
        });
        self.tasks.push(task);
    }

    /// Add relays and connect to them.
    pub fn add_relays(&mut self, urls: Vec<String>, cx: &mut Context<Self>) {
        let backend = self.inner.clone();

        let task = cx.background_spawn(async move {
            for url in urls {
                backend.add_relay(&url).await?;
            }
            backend.connect().await;
            Ok::<(), Error>(())
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            match task.await {
                Ok(()) => {
                    this.update(cx, |_this, cx| cx.emit(BackendEvent::Connected))?;
                }
                Err(e) => {
                    this.update(cx, |_this, cx| cx.emit(BackendEvent::error(e.to_string())))?;
                }
            }
            Ok(())
        }));
    }

    /// Add relays used only for discovery (e.g. NIP-65 indexers) and
    /// connect to them. No subscriptions or writes are routed through them.
    pub fn add_discovery_relays(&mut self, urls: Vec<String>, cx: &mut Context<Self>) {
        let backend = self.inner.clone();

        let task = cx.background_spawn(async move {
            for url in urls {
                backend.add_discovery_relay(&url).await?;
            }
            backend.connect().await;
            Ok::<(), Error>(())
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            if let Err(e) = task.await {
                this.update(cx, |_this, cx| cx.emit(BackendEvent::error(e.to_string())))?;
            }
            Ok(())
        }));
    }

    /// Start a persistent subscription. Matching events are stored in the
    /// database automatically and surface as [`BackendEvent::NostrUpdate`].
    pub fn subscribe(&mut self, filter: Filter, cx: &mut Context<Self>) {
        let backend = self.inner.clone();

        let task = cx.background_spawn(async move { backend.subscribe(filter).await.map(|_| ()) });

        self.tasks.push(cx.spawn(async move |this, cx| {
            if let Err(e) = task.await {
                this.update(cx, |_this, cx| cx.emit(BackendEvent::error(e.to_string())))?;
            }
            Ok(())
        }));
    }

    /// Sign, broadcast and locally store an event. Emits
    /// [`BackendEvent::Published`] on success so stores can refresh.
    ///
    /// The returned receiver yields the outcome of this specific action,
    /// so callers can show inline progress/errors instead of relying on
    /// the global [`BackendEvent::Error`].
    pub fn send(
        &mut self,
        builder: EventBuilder,
        cx: &mut Context<Self>,
    ) -> flume::Receiver<Result<Event, Error>> {
        let (tx, rx) = flume::bounded(1);

        let backend = self.inner.clone();
        let task = cx.background_spawn(async move { backend.send(builder).await });

        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = task.await;

            match &result {
                Ok(event) => {
                    this.update(cx, |_this, cx| {
                        cx.emit(BackendEvent::Published(Box::new(event.clone())));
                    })?;
                }
                Err(e) => {
                    this.update(cx, |_this, cx| cx.emit(BackendEvent::error(e.to_string())))?;
                }
            }

            tx.send_async(result)
                .await
                .map_err(|_| anyhow!("action result receiver dropped"))
        }));

        rx
    }
}
