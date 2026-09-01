use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant};

use anyhow::{Error, anyhow, bail};
use bitcoin_hashes::sha1::Hash as Sha1Hash;
use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use nostr::event::IntoEventBuilder;
use nostr_connect::prelude::*;
use nostr_sdk::client::SyncSummary;
use nostr_sdk::prelude::*;
use signed_core::{Announcement, RepoAddr, build_state, filters, identifier_from_name, repo_addr};
use signed_nostr::{SignedAuthUrlHandler, UniversalSigner, Update};

use crate::git_store::GitStore;

/// Keyring entry holding the user credential (`nsec1...` or `bunker://...`
/// with an embedded `?master=<nsec>` NIP-46 session key).
pub const USER_KEYRING: &str = "Signed Safe Storage";
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
pub const INDEXER_RELAYS: [&str; 2] = ["wss://indexer.coracle.social", "wss://user.kindpag.es"];

/// How long an identical fetch/sync request is suppressed after it started.
/// A second panel for the same repository (or the global and per-author
/// list stores at login) doesn't duplicate a sync that just ran; after the
/// window, re-fetching is allowed again so data stays fresh.
const FETCH_DEDUP_WINDOW: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone)]
pub enum BackendEvent {
    /// User has no signer configured.
    SignerRequired,
    /// The stored identity is NIP-49 encrypted (`ncryptsec1...`); a
    /// passphrase is required to decrypt it before the session can resume.
    PassphraseRequired,
    /// The signer has changed (login/logout/account switch).
    SignerChanged,
    /// Relay bootstrap finished.
    Connected,
    /// A new event was received from a relay and stored in the database.
    NostrUpdate(Update),
    /// A negentropy sync completed; the database was updated directly,
    /// so stores should re-query (no [`BackendEvent::NostrUpdate`] is fired
    /// for synced events).
    Synced,
    /// A negentropy sync is in flight. Stores may re-query to render
    /// incrementally; UI can show `current`/`total` progress.
    SyncProgress {
        /// Total events to process.
        total: u64,
        /// Events processed so far.
        current: u64,
    },
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
    client: Client,
    signer: UniversalSigner,
    current_user: Option<PublicKey>,
    connected: bool,
    sync_progress: Option<(u64, u64)>,
    /// Whether the stored credential is NIP-49 encrypted and a passphrase
    /// is still needed to resume the session.
    passphrase_required: bool,
    /// Fingerprints of recently started fetches/syncs (relay + filter set),
    /// so duplicate requests within [`FETCH_DEDUP_WINDOW`] collapse into
    /// one. Entries are pruned lazily on the next request.
    recent_fetches: HashMap<u64, Instant>,
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

    pub(crate) fn new(client: Client, signer: UniversalSigner, cx: &mut Context<Self>) -> Self {
        let pump_client = client.clone();

        let pump = cx.spawn(async move |this, cx| {
            let mut notifications = pump_client.notifications();

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
            client,
            signer,
            current_user: None,
            connected: false,
            sync_progress: None,
            passphrase_required: false,
            recent_fetches: HashMap::new(),
            tasks: vec![pump],
        };

        this.bootstrap(cx);
        this
    }

    /// Bootstrap the client: connect to the default relays (indexers as
    /// discovery-only) and restore the saved session, if any.
    fn bootstrap(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();

        let task = cx.background_spawn(async move {
            for url in BOOTSTRAP_RELAYS {
                client.add_relay(url).await?;
            }
            for url in INDEXER_RELAYS {
                client
                    .add_relay(url)
                    .capabilities(RelayCapabilities::DISCOVERY)
                    .await?;
            }
            client.connect().await;
            Ok::<(), Error>(())
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            match task.await {
                Ok(()) => {
                    this.update(cx, |this, cx| {
                        this.connected = true;
                        cx.emit(BackendEvent::Connected);
                        cx.notify();
                    })?;
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
    /// [`BackendEvent::SignerRequired`] if no credential is stored, or
    /// [`BackendEvent::PassphraseRequired`] if the stored identity is
    /// NIP-49 encrypted.
    pub fn restore_session(&mut self, cx: &mut Context<Self>) {
        if cfg!(target_arch = "wasm32") {
            cx.emit(BackendEvent::SignerRequired);
            return;
        }

        let user = cx.read_credentials(USER_KEYRING);

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
                    let (base, keys) = extract_master_key(&content);
                    let uri = NostrConnectUri::parse(base)?;
                    let mut signer = NostrConnect::new(
                        uri,
                        keys,
                        Duration::from_secs(NOSTR_CONNECT_TIMEOUT),
                        None,
                    )?;
                    signer.auth_url_handler(SignedAuthUrlHandler);
                    this.update(cx, |this, cx| this.set_signer(signer, cx))?;
                } else if content.starts_with("ncryptsec1") {
                    // Encrypted identity: a passphrase is required to
                    // decrypt it before the session can resume.
                    log::warn!("stored identity is ncryptsec-encrypted; waiting for passphrase");
                    this.update(cx, |this, cx| {
                        this.passphrase_required = true;
                        cx.emit(BackendEvent::PassphraseRequired);
                    })?;
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

    /// Decrypt the NIP-49 encrypted credential stored in the keyring with
    /// the given passphrase and resume the session.
    ///
    /// The scrypt decryption runs off the UI thread. The task yields the
    /// public key, or the failure reason (e.g. wrong passphrase).
    pub fn restore_with_passphrase(
        &mut self,
        password: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<PublicKey, Error>> {
        let password = password.to_owned();
        let user = cx.read_credentials(USER_KEYRING);

        cx.spawn(async move |this, cx| {
            let content = user
                .await?
                .map(|(_username, secret)| String::from_utf8(secret))
                .transpose()?
                .ok_or_else(|| anyhow!("no stored credential; nothing to unlock"))?;

            if !content.starts_with("ncryptsec1") {
                return Err(anyhow!("stored credential is not passphrase-encrypted"));
            }

            let decrypt_task = cx.background_spawn(async move {
                let encrypted = EncryptedSecretKey::from_bech32(&content)?;
                let secret = encrypted.decrypt(&password)?;
                Ok::<_, Error>(Keys::new(secret))
            });

            let keys = decrypt_task.await?;
            let public_key = keys.public_key();

            this.update(cx, |this, cx| this.set_signer(keys, cx))?;

            Ok(public_key)
        })
    }

    /// Create a new identity: generate keys, encrypt the secret key with the
    /// passphrase (NIP-49) and persist it in the keyring, then publish the
    /// user's NIP-65 relay list, metadata and grasp list.
    ///
    /// The encryption runs off the UI thread; the task yields the new
    /// public key.
    pub fn create_identity(
        &mut self,
        name: &str,
        password: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<PublicKey, Error>> {
        let name = name.trim().to_owned();
        let password = password.to_owned();

        if name.is_empty() || name.len() > 255 {
            return Task::ready(Err(anyhow!("Name must be 1-255 characters")));
        }
        if password.is_empty() {
            return Task::ready(Err(anyhow!("Passphrase must not be empty")));
        }

        cx.spawn(async move |this, cx| {
            let job = cx.background_spawn(async move {
                let keys = Keys::generate();
                let encrypted =
                    EncryptedSecretKey::new(keys.secret_key(), &password, 16, KeySecurity::Medium)?;
                let ncryptsec = encrypted.to_bech32()?;
                Ok::<_, Error>((keys, ncryptsec))
            });

            let (keys, ncryptsec) = job.await?;
            let public_key = keys.public_key();

            // Persist the encrypted credential.
            let write = cx.update(|cx| {
                cx.write_credentials(USER_KEYRING, &public_key.to_hex(), ncryptsec.as_bytes())
            });
            write.await?;

            this.update(cx, |this, cx| {
                // Become the new identity, so the publishes below are
                // signed with the new keys.
                this.signer.swap_inner(keys);
                this.current_user = Some(public_key);
                this.bootstrap_user(public_key, cx);
                cx.emit(BackendEvent::SignerChanged);
                cx.notify();

                let relays: Vec<(RelayUrl, Option<RelayMetadata>)> = [
                    (
                        RelayUrl::parse("wss://relay.primal.net").unwrap(),
                        Some(RelayMetadata::Read),
                    ),
                    (
                        RelayUrl::parse("wss://relay.ditto.pub").unwrap(),
                        Some(RelayMetadata::Read),
                    ),
                    (
                        RelayUrl::parse("wss://relay.nostr.net").unwrap(),
                        Some(RelayMetadata::Write),
                    ),
                    (
                        RelayUrl::parse("wss://nos.lol").unwrap(),
                        Some(RelayMetadata::Write),
                    ),
                ]
                .to_vec();

                this.send_fire_and_forget(RelayList::new(relays).into_event_builder(), cx);

                let metadata = Metadata::new()
                    .name(&name)
                    .display_name(&name)
                    .into_event_builder();

                this.send_fire_and_forget(metadata, cx);

                let grasp_servers: Vec<RelayUrl> = ["wss://gitnostr.com", "wss://relay.ngit.dev"]
                    .into_iter()
                    .map(|url| RelayUrl::parse(url).expect("valid relay URL"))
                    .collect();

                this.send_fire_and_forget(
                    GitUserGraspList { grasp_servers }.into_event_builder(),
                    cx,
                );
            })?;

            Ok(public_key)
        })
    }

    /// Create a new repository: initialize a local clone with a `main`
    /// branch and a `README.md`, publish the NIP-34 announcement and the
    /// repository state to the grasp relays, then push the initial commit
    /// to each grasp server.
    ///
    /// The events must reach the grasp servers *before* the push: GRASP
    /// servers hold the signed state event in "purgatory" and only accept
    /// a push for a not-yet-existing repository while that authorization is
    /// pending (it expires after 30 minutes), like gitworkshop and ngit.
    ///
    /// The git work runs on background threads; the task yields the
    /// published announcement.
    pub fn create_repository(
        &mut self,
        name: &str,
        description: &str,
        grasp_servers: Vec<RelayUrl>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Announcement, Error>> {
        let name = name.trim().to_owned();
        let description = description.trim().to_owned();

        if name.is_empty() {
            return Task::ready(Err(anyhow!("Repository name is required")));
        }
        if grasp_servers.is_empty() {
            return Task::ready(Err(anyhow!("Add at least one grasp server")));
        }
        let Some(public_key) = self.current_user else {
            return Task::ready(Err(anyhow!("Sign in to create a repository")));
        };

        // The repository identifier is derived from the name, like ngit and
        // gitworkshop: spaces become hyphens, other non-alphanumeric
        // characters (except `/`) become hyphens, case is preserved.
        let repo_id = identifier_from_name(&name);
        if repo_id.is_empty() || repo_id.len() > 100 {
            return Task::ready(Err(anyhow!(
                "Repository name must produce an identifier of 1-100 characters"
            )));
        }
        if !repo_id.chars().any(|c| c.is_ascii_alphanumeric()) {
            return Task::ready(Err(anyhow!(
                "Repository name must contain at least one alphanumeric character"
            )));
        }

        let addr = repo_addr(public_key, repo_id.clone());
        let cache = GitStore::global(cx).cache().clone();
        let path = cache.repo_path(&addr);
        let owner = public_key
            .to_bech32()
            .unwrap_or_else(|_| public_key.to_hex());
        let servers = grasp_servers.clone();

        cx.spawn(async move |this, cx| {
            // Initialize the local clone (main branch + README + initial commit).
            let work = cx.background_spawn({
                let path = path.clone();
                let name = name.clone();
                let description = description.clone();
                let owner = owner.clone();
                let repo_id = repo_id.clone();
                let servers = servers.clone();

                async move {
                    let parent = path
                        .parent()
                        .ok_or_else(|| anyhow!("invalid repository path"))?;
                    std::fs::create_dir_all(parent)?;
                    let commit = signed_git::init_repository(&path, &name, &description)?;

                    // Point `origin` at the first grasp server so later
                    // fetches (and pushes) have a target, like ngit.
                    if let Some(base) = servers.first().and_then(grasp_base_url) {
                        let url = format!("{base}/{owner}/{repo_id}.git");
                        signed_git::ensure_origin(&path, &url).ok();
                    }

                    Ok::<_, Error>(commit)
                }
            });

            let commit = work.await?;
            let commit_sha =
                Sha1Hash::from_str(&commit).map_err(|_| anyhow!("invalid initial commit id"))?;

            // The nostr client queues events until each relay is connected.
            this.update(cx, |this, cx| {
                let urls: Vec<String> = servers.iter().map(ToString::to_string).collect();
                this.add_relays(urls, cx);
            })?;

            // The state event is the push authorization ("purgatory"), so
            // it must be accepted before the push below.
            let announcement = GitRepositoryAnnouncement {
                id: repo_id.clone(),
                name: Some(name.clone()),
                description: (!description.is_empty()).then_some(description.clone()),
                web: Vec::new(),
                clone: servers
                    .iter()
                    .filter_map(|relay| grasp_clone_url(relay, &owner, &repo_id))
                    .collect(),
                relays: servers.clone(),
                euc: Some(commit_sha),
                maintainers: Vec::new(),
            };

            let event = this
                .update(cx, |this, cx| {
                    this.send(announcement.into_event_builder(), cx)
                })?
                .await?;

            let state_event = match this
                .update(cx, |this, cx| {
                    let builder = build_state(
                        &repo_id,
                        &[("refs/heads/main".to_owned(), commit)],
                        Some("main"),
                    );
                    this.send(builder, cx)
                })?
                .await
            {
                Ok(state_event) => state_event,
                Err(e) => {
                    this.update(cx, |this, cx| {
                        this.retract_events(std::slice::from_ref(&event), cx);
                    })
                    .ok();

                    return Err(e.context(
                        "The repository was announced, but its state could not be published. \
                         The announcement has been retracted",
                    ));
                }
            };

            // Push to every grasp server; creation only fails when no
            // server accepted it.
            let push = cx.background_spawn({
                let path = path.clone();
                let owner = owner.clone();
                let repo_id = repo_id.clone();
                let servers = servers.clone();
                push_to_grasp_servers(path, owner, repo_id, servers, signed_git::push_main)
            });
            if let Err(e) = push.await {
                // The events are already published; retract them so the
                // repository doesn't remain announced without content.
                this.update(cx, |this, cx| {
                    this.retract_events(&[event.clone(), state_event.clone()], cx);
                })
                .ok();

                return Err(e.context(
                    "The repository was announced, but the push to every grasp server failed. \
                     The announcement has been retracted",
                ));
            }

            Announcement::from_event(&event).ok_or_else(|| anyhow!("failed to parse announcement"))
        })
    }

    /// Publish an existing local repository to NIP-34: read its current
    /// branches, tags and HEAD, publish the announcement and the repository
    /// state to the grasp relays, then push every branch and tag to each
    /// grasp server. Also points `origin` at the first grasp server.
    ///
    /// Same ordering constraint as [`Self::create_repository`]: the state
    /// event ("purgatory") must be accepted before the push.
    pub fn publish_local_repo(
        &mut self,
        path: PathBuf,
        name: &str,
        description: &str,
        grasp_servers: Vec<RelayUrl>,
        cx: &mut Context<Self>,
    ) -> Task<Result<Announcement, Error>> {
        let name = name.trim().to_owned();
        let description = description.trim().to_owned();

        if name.is_empty() {
            return Task::ready(Err(anyhow!("Repository name is required")));
        }

        if grasp_servers.is_empty() {
            return Task::ready(Err(anyhow!("Add at least one grasp server")));
        }

        let Some(public_key) = self.current_user else {
            return Task::ready(Err(anyhow!("Sign in to publish a repository")));
        };

        // The repository identifier is derived from the name as in
        // [`Self::create_repository`].
        let repo_id = identifier_from_name(&name);

        if repo_id.is_empty() || repo_id.len() > 100 {
            return Task::ready(Err(anyhow!(
                "Repository name must produce an identifier of 1-100 characters"
            )));
        }

        if !repo_id.chars().any(|c| c.is_ascii_alphanumeric()) {
            return Task::ready(Err(anyhow!(
                "Repository name must contain at least one alphanumeric character"
            )));
        }

        let owner = public_key.to_bech32().unwrap();
        let servers = grasp_servers.clone();

        cx.spawn(async move |this, cx| {
            let work = cx.background_spawn({
                let path = path.clone();
                async move {
                    let state = signed_git::worktree_ref_state(&path)?;
                    let euc = signed_git::root_commit(&path)?;
                    Ok::<_, Error>((state, euc))
                }
            });
            let (state, euc) = work.await?;

            // The nostr client queues events until each relay is connected.
            this.update(cx, |this, cx| {
                let urls: Vec<String> = servers.iter().map(ToString::to_string).collect();
                this.add_relays(urls, cx);
            })?;

            // The state event is the push authorization ("purgatory"), so
            // it must be accepted before the push below.
            let announcement = GitRepositoryAnnouncement {
                id: repo_id.clone(),
                name: Some(name.clone()),
                description: (!description.is_empty()).then_some(description.clone()),
                web: Vec::new(),
                clone: servers
                    .iter()
                    .filter_map(|relay| grasp_clone_url(relay, &owner, &repo_id))
                    .collect(),
                relays: servers.clone(),
                euc: euc.and_then(|commit| Sha1Hash::from_str(&commit).ok()),
                maintainers: Vec::new(),
            };

            let event = this
                .update(cx, |this, cx| {
                    this.send(announcement.into_event_builder(), cx)
                })?
                .await?;

            let refs = state.refs.clone();
            let head = state.head.clone();
            let state_event = match this
                .update(cx, |this, cx| {
                    let builder = build_state(&repo_id, &refs, head.as_deref());
                    this.send(builder, cx)
                })?
                .await
            {
                Ok(state_event) => state_event,
                Err(e) => {
                    this.update(cx, |this, cx| {
                        this.retract_events(std::slice::from_ref(&event), cx);
                    })
                    .ok();

                    return Err(e.context(
                        "The repository was announced, but its state could not be published. \
                         The announcement has been retracted",
                    ));
                }
            };

            // Push every branch and tag to each grasp server; the init
            // only fails when no server accepted it. An empty repository
            // has nothing to push.
            if !refs.is_empty() {
                let push = cx.background_spawn({
                    let path = path.clone();
                    let owner = owner.clone();
                    let repo_id = repo_id.clone();
                    let servers = servers.clone();
                    push_to_grasp_servers(path, owner, repo_id, servers, signed_git::push_all)
                });
                if let Err(e) = push.await {
                    this.update(cx, |this, cx| {
                        this.retract_events(&[event.clone(), state_event.clone()], cx);
                    })
                    .ok();

                    return Err(e.context(
                        "The repository was announced, but the push to every grasp server failed. \
                         The announcement has been retracted",
                    ));
                }
            }

            // Point `origin` at the first grasp server so later pushes
            // have a target.
            if let Some(base) = servers.first().and_then(grasp_base_url) {
                let url = format!("{base}/{owner}/{repo_id}.git");
                let path = path.clone();
                cx.background_spawn(async move {
                    signed_git::ensure_origin(&path, &url).ok();
                })
                .await;
            }

            Announcement::from_event(&event).ok_or_else(|| anyhow!("failed to parse announcement"))
        })
    }

    /// Re-push the repository's current refs to the grasp servers announced
    /// in its `relays` tag: publishes a fresh state event (the push
    /// authorization), then pushes every branch and tag, like the init
    /// flow. The repository must have a local clone in the cache.
    pub fn push_repository(
        &mut self,
        announcement: Announcement,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), Error>> {
        let addr = announcement.addr();
        let cache = GitStore::global(cx).cache().clone();
        let path = cache.repo_path(&addr);
        let owner = announcement
            .owner
            .to_bech32()
            .unwrap_or_else(|_| announcement.owner.to_hex());
        let repo_id = announcement.id.clone();
        let relays = announcement.relays.clone();

        cx.spawn(async move |this, cx| {
            let work = cx.background_spawn({
                let path = path.clone();
                async move { signed_git::worktree_ref_state(&path) }
            });
            let state = work.await?;

            // Grasp servers authorize a push by the state they have seen.
            let refs = state.refs.clone();
            let head = state.head.clone();
            this.update(cx, |this, cx| {
                let builder = build_state(&repo_id, &refs, head.as_deref());
                this.send(builder, cx)
            })?
            .await?;

            if !refs.is_empty() {
                let push = cx.background_spawn({
                    let path = path.clone();
                    let owner = owner.clone();
                    let repo_id = repo_id.clone();
                    let relays = relays.clone();
                    async move {
                        push_to_grasp_servers(path, owner, repo_id, relays, signed_git::push_all)
                            .await
                    }
                });
                push.await?;
            }

            Ok(())
        })
    }

    /// Delete the repository from nostr: publish NIP-09 deletions for its
    /// announcement, state and activity events (issues, pull requests,
    /// patches, statuses, comments). Only the repository owner may delete
    /// it.
    pub fn delete_repository(
        &mut self,
        addr: RepoAddr,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), Error>> {
        let Some(public_key) = self.current_user else {
            return Task::ready(Err(anyhow!("Sign in to delete a repository")));
        };
        if public_key != addr.public_key {
            return Task::ready(Err(anyhow!("Only the repository owner can delete it")));
        }

        let client = self.client.clone();
        let addr = addr.clone();

        cx.spawn(async move |this, cx| {
            // Collect every event of the repository from the local database.
            let events = cx.background_spawn(async move {
                let db = client.database();
                let mut events = Vec::new();
                for filter in [
                    filters::announcement(&addr),
                    filters::state(&addr),
                    filters::activity(&addr),
                ] {
                    events.extend(db.query(filter).await?);
                }
                Ok::<_, Error>(events)
            });
            let events = events.await?;

            this.update(cx, |this, cx| {
                this.retract_events(&events, cx);
            })
            .ok();

            Ok(())
        })
    }

    /// Login with an `nsec1...` key or a `bunker://...` URI, dispatching on
    /// the credential's prefix.
    pub fn login(&mut self, credential: &str, cx: &mut Context<Self>) {
        let credential = credential.trim();

        if credential.starts_with("nsec1") {
            self.login_with_nsec(credential, cx);
        } else if credential.starts_with("bunker://") {
            self.login_with_bunker(credential, cx);
        } else {
            cx.emit(BackendEvent::error(
                "Unsupported credential, expected nsec1... or bunker://...",
            ));
        }
    }

    /// Create a fresh identity and login with it. The generated key is
    /// persisted in the keyring like any other `nsec` credential.
    pub fn login_with_new_identity(&mut self, cx: &mut Context<Self>) {
        let nsec = Keys::generate()
            .secret_key()
            .to_bech32()
            .expect("infallible");
        self.login_with_nsec(&nsec, cx);
    }

    /// Login with an `nsec1...` secret key. The credential is verified by
    /// the signer flow and persisted in the keyring.
    pub fn login_with_nsec(&mut self, nsec: &str, cx: &mut Context<Self>) {
        let keys = match SecretKey::parse(nsec) {
            Ok(secret) => Keys::new(secret),
            Err(e) => {
                cx.emit(BackendEvent::error(e.to_string()));
                return;
            }
        };

        let nsec = nsec.trim().to_owned();
        let pubkey = keys.public_key().to_hex();
        let write = cx.write_credentials(USER_KEYRING, &pubkey, nsec.as_bytes());

        self.tasks.push(cx.spawn(async move |this, cx| {
            if let Err(e) = write.await {
                this.update(cx, |_, cx| cx.emit(BackendEvent::error(e.to_string())))?;
                return Ok(());
            }
            this.update(cx, |this, cx| this.set_signer(keys, cx))?;
            Ok(())
        }));
    }

    /// Login with a `bunker://...` URI (NIP-46). A fresh session key is
    /// generated and embedded into the stored URI as `?master=<nsec>`, so
    /// no separate keyring entry is needed. The auth URL, if any, is opened
    /// in the default browser. The credential is persisted in the keyring
    /// after the signer proves reachable.
    pub fn login_with_bunker(&mut self, uri: &str, cx: &mut Context<Self>) {
        let uri_string = uri.trim().to_owned();

        let connect_uri = match NostrConnectUri::parse(&uri_string) {
            Ok(uri) => uri,
            Err(e) => {
                cx.emit(BackendEvent::error(e.to_string()));
                return;
            }
        };

        let keys = Keys::generate();
        let credential = with_master_key(&uri_string, &keys);
        let write = cx.write_credentials(USER_KEYRING, "bunker", credential.as_bytes());

        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = async {
                let mut signer = NostrConnect::new(
                    connect_uri,
                    keys,
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
                this.signer.swap_inner(Keys::generate());
                this.current_user = None;
                this.passphrase_required = false;
                cx.emit(BackendEvent::SignerChanged);
                cx.emit(BackendEvent::SignerRequired);
                cx.notify();
            })?;

            Ok(())
        }));
    }

    /// Fetch the user's grasp list (kind `10317`) and add the listed grasp
    /// servers as relays.
    fn bootstrap_user(&mut self, public_key: PublicKey, cx: &mut Context<Self>) {
        let client = self.client.clone();

        self.tasks.push(cx.spawn(async move |this, cx| {
            let result = async {
                let events = client.fetch_events(filters::grasp_list(public_key)).await?;

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
                    client.add_relay(&url).await.ok();
                }
                client.connect().await;

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
        self.client.clone()
    }

    /// Get the current signer.
    pub fn signer(&self) -> UniversalSigner {
        self.signer.clone()
    }

    /// Get the current user's public key.
    pub fn current_user(&self) -> Option<PublicKey> {
        self.current_user
    }

    /// Whether the stored credential is NIP-49 encrypted and a passphrase
    /// is still needed to resume the session.
    pub fn passphrase_required(&self) -> bool {
        self.passphrase_required
    }

    /// Surface an error message through [`BackendEvent::Error`].
    pub fn emit_error(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        cx.emit(BackendEvent::error(message));
    }

    /// Whether the relay bootstrap has completed.
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Progress of the in-flight negentropy sync, if any: `(total, current)`.
    pub fn sync_progress(&self) -> Option<(u64, u64)> {
        self.sync_progress
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
                        this.signer.swap_inner(new_signer);
                        this.current_user = Some(public_key);
                        this.passphrase_required = false;
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
        let client = self.client.clone();

        let task = cx.background_spawn(async move {
            for url in urls {
                client.add_relay(&url).await?;
            }
            client.connect().await;
            Ok::<(), Error>(())
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            match task.await {
                Ok(()) => {
                    this.update(cx, |this, cx| {
                        this.connected = true;
                        cx.emit(BackendEvent::Connected);
                        cx.notify();
                    })?;
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
        let client = self.client.clone();

        let task = cx.background_spawn(async move {
            for url in urls {
                client
                    .add_relay(&url)
                    .capabilities(RelayCapabilities::DISCOVERY)
                    .await?;
            }
            client.connect().await;
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
        let client = self.client.clone();

        let task = cx.background_spawn(async move { client.subscribe(filter).await.map(|_| ()) });

        self.tasks.push(cx.spawn(async move |this, cx| {
            if let Err(e) = task.await {
                this.update(cx, |_this, cx| cx.emit(BackendEvent::error(e.to_string())))?;
            }
            Ok(())
        }));
    }

    /// Whether an identical fetch was started within [`FETCH_DEDUP_WINDOW`]
    /// and is still recent enough to suppress a duplicate. Records the
    /// fingerprint (after pruning expired entries) when returning `false`.
    fn fetch_recently_started(&mut self, fingerprint: u64) -> bool {
        self.recent_fetches
            .retain(|_, started| started.elapsed() < FETCH_DEDUP_WINDOW);
        if self.recent_fetches.contains_key(&fingerprint) {
            return true;
        }
        self.recent_fetches.insert(fingerprint, Instant::now());
        false
    }

    /// Connect to relays announced by a repository (NIP-34 `relays` tag) and
    /// fetch its events from them: a one-shot auto-closing subscription for
    /// `filters`, plus a negentropy sync so issues, patches and PRs stored
    /// only on those relays are not missed.
    ///
    /// Deduplicated: an identical request (same relays and filters) started
    /// within [`FETCH_DEDUP_WINDOW`] is skipped, so a second panel for the
    /// same repository doesn't re-run the fetch.
    ///
    /// Best-effort: failures are logged, not surfaced. The relays stay in
    /// the pool, so later publishes for this repository also reach them.
    pub fn connect_repo_relays(
        &mut self,
        relays: Vec<RelayUrl>,
        filters: Vec<Filter>,
        cx: &mut Context<Self>,
    ) {
        let relay_strs: Vec<&str> = relays.iter().map(|url| url.as_str()).collect();
        let fingerprint = fetch_fingerprint(&relay_strs, &filters);
        if self.fetch_recently_started(fingerprint) {
            log::debug!("skipping duplicate repo relay fetch");
            return;
        }

        let client = self.client.clone();

        self.tasks.push(cx.spawn(async move |this, cx| {
            if let Err(e) = connect_repo_relays_only(&client, relays, filters).await {
                log::warn!("repo relay fetch failed: {e}");
                // Allow an immediate retry after a failure.
                this.update(cx, |this, _cx| {
                    this.recent_fetches.remove(&fingerprint);
                })
                .ok();
            }
            Ok(())
        }));
    }

    /// Start a one-shot subscription targeted only at the bootstrap relays,
    /// auto-closing after EOSE or a short timeout. Matching events are stored
    /// in the database and surface as [`BackendEvent::NostrUpdate`] while the
    /// subscription is open.
    pub fn subscribe_bootstrap(&mut self, filters: Vec<Filter>, cx: &mut Context<Self>) {
        let client = self.client.clone();

        let task =
            cx.background_spawn(async move { subscribe_bootstrap_only(&client, filters).await });

        self.tasks.push(cx.spawn(async move |this, cx| {
            if let Err(e) = task.await {
                this.update(cx, |_this, cx| cx.emit(BackendEvent::error(e.to_string())))?;
            }
            Ok(())
        }));
    }

    /// Negentropy-sync the given filter against the bootstrap relays:
    /// reconciles the local database with the relays in both directions.
    /// Emits [`BackendEvent::SyncProgress`] while running (throttled to
    /// whole-percent changes) and [`BackendEvent::Synced`] on completion.
    ///
    /// Deduplicated: an identical sync started within
    /// [`FETCH_DEDUP_WINDOW`] is skipped. Observers still see the original
    /// sync's progress and completion events.
    pub fn sync_bootstrap(&mut self, filter: Filter, cx: &mut Context<Self>) {
        let fingerprint = fetch_fingerprint(&BOOTSTRAP_RELAYS, std::slice::from_ref(&filter));
        if self.fetch_recently_started(fingerprint) {
            log::debug!("skipping duplicate bootstrap sync");
            return;
        }

        let client = self.client.clone();

        self.sync_progress = Some((0, 0));
        cx.notify();

        let (tx, mut rx) = SyncProgress::channel();

        self.tasks.push(cx.spawn(async move |this, cx| {
            let mut last_percent: u64 = 0;

            while rx.changed().await.is_ok() {
                let progress = *rx.borrow_and_update();
                let percent = (progress.percentage() * 100.0) as u64;

                if progress.current > 0 && percent != last_percent {
                    last_percent = percent;

                    let alive = this.update(cx, |this, cx| {
                        this.sync_progress = Some((progress.total, progress.current));
                        cx.emit(BackendEvent::SyncProgress {
                            total: progress.total,
                            current: progress.current,
                        });
                        cx.notify();
                    });

                    if alive.is_err() {
                        break;
                    }
                }
            }

            Ok(())
        }));

        let task = cx.background_spawn(async move {
            let opts = SyncOptions::default().progress(tx);
            sync_bootstrap_only(&client, filter, opts).await
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            match task.await {
                Ok(summary) => {
                    log::debug!(
                        "sync done: {} received, {} sent",
                        summary.received.len(),
                        summary.sent.len()
                    );
                    this.update(cx, |this, cx| {
                        this.sync_progress = None;
                        cx.emit(BackendEvent::Synced);
                        cx.notify();
                    })?;
                }
                Err(e) => {
                    this.update(cx, |this, cx| {
                        this.sync_progress = None;
                        // Allow an immediate retry after a failure.
                        this.recent_fetches.remove(&fingerprint);
                        cx.emit(BackendEvent::error(e.to_string()))
                    })?;
                }
            }
            Ok(())
        }));
    }

    /// Sign, broadcast and locally store an event. Emits
    /// [`BackendEvent::Published`] on success so stores can refresh.
    ///
    /// The task yields the outcome of this specific action (for inline
    /// progress/errors) and is owned by the caller; dropping it cancels
    /// the publish.
    pub fn send(
        &mut self,
        builder: EventBuilder,
        cx: &mut Context<Self>,
    ) -> Task<Result<Event, Error>> {
        let client = self.client.clone();
        let signer = self.signer.clone();

        cx.spawn(async move |this, cx| {
            // Sign with the current signer, broadcast, and save locally so
            // the event is immediately visible to database queries.
            let work = cx.background_spawn(async move {
                let event = builder.finalize_async(&signer).await?;
                let output = client.send_event(&event).await?;

                if output.success.is_empty() && !output.failed.is_empty() {
                    let reasons = output
                        .failed
                        .values()
                        .cloned()
                        .collect::<Vec<String>>()
                        .join(", ");
                    return Err(anyhow!("event not accepted by any relay: {reasons}"));
                }

                Ok(event)
            });

            let result = work.await;

            match &result {
                Ok(event) => {
                    this.update(cx, |_this, cx| {
                        cx.emit(BackendEvent::Published(Box::new(event.clone())));
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |_this, cx| {
                        cx.emit(BackendEvent::error(e.to_string()));
                    })
                    .ok();
                }
            }

            result
        })
    }

    /// Publish a NIP-34 repository announcement (kind 30617) with the
    /// current signer. The returned task yields the published event, so
    /// callers can show inline progress/errors.
    pub fn publish_announcement(
        &mut self,
        announcement: GitRepositoryAnnouncement,
        cx: &mut Context<Self>,
    ) -> Task<Result<Event, Error>> {
        self.send(announcement.into_event_builder(), cx)
    }

    /// Sign, broadcast and store an event without awaiting the result;
    /// failures surface through [`BackendEvent::Error`]. The spawned task is
    /// owned by the backend, so it is cancelled when the backend is dropped.
    fn send_fire_and_forget(&mut self, builder: EventBuilder, cx: &mut Context<Self>) {
        let task = self.send(builder, cx);

        self.tasks.push(cx.spawn(async move |this, cx| {
            if let Err(e) = task.await {
                this.update(cx, |_this, cx| {
                    cx.emit(BackendEvent::error(e.to_string()));
                })
                .ok();
            }
            Ok(())
        }));
    }

    /// Publish a NIP-09 deletion event for `events` (best-effort), so a
    /// publish that fails midway can retract the events that were already
    /// broadcast to relays. Failures are logged, not surfaced: the caller's
    /// error already told the user what happened.
    fn retract_events(&mut self, events: &[Event], cx: &mut Context<Self>) {
        if events.is_empty() {
            return;
        }

        let mut tags: Vec<Tag> = Vec::with_capacity(events.len() * 2);

        for event in events {
            tags.push(Tag::event(event.id));
            tags.push(Tag::parse(["k", &event.kind.to_string()]).expect("valid kind tag"));
        }

        let task = self.send(EventBuilder::new(Kind::EventDeletion, "").tags(tags), cx);

        self.tasks.push(cx.spawn(async move |_this, _cx| {
            if let Err(e) = task.await {
                log::warn!("failed to retract repository events: {e}");
            }
            Ok(())
        }));
    }
}

/// Fingerprint of a relay + filter set, for fetch dedup. Relays and
/// filters are sorted first so the fingerprint is order-independent.
fn fetch_fingerprint(relays: &[&str], filters: &[Filter]) -> u64 {
    let mut relays: Vec<&str> = relays.to_vec();
    relays.sort_unstable();
    let mut filters: Vec<&Filter> = filters.iter().collect();
    filters.sort_unstable();

    let mut hasher = DefaultHasher::new();
    relays.hash(&mut hasher);
    filters.hash(&mut hasher);
    hasher.finish()
}

/// Add the given relays, connect to them, and fetch the filters: a one-shot
/// subscription (auto-closing after EOSE) plus a negentropy sync per filter
/// as a second pass, so events that race with the subscription or relays
/// with flaky EOSE behavior can't be missed. Relays without NEG-XX support
/// just fail the sync step; the subscription already covered them.
async fn connect_repo_relays_only(
    client: &Client,
    relays: Vec<RelayUrl>,
    filters: Vec<Filter>,
) -> Result<(), Error> {
    if relays.is_empty() {
        return Ok(());
    }

    let mut added = false;
    for url in &relays {
        added |= client.add_relay(url).await?;
    }
    // Connecting is only needed when the pool grew; connected relays no-op,
    // but the call still iterates every relay in the pool.
    if added {
        client.connect().await;
    }

    let opts = SubscribeAutoCloseOptions::default()
        .exit_policy(ReqExitPolicy::ExitOnEOSE)
        .timeout(Some(Duration::from_secs(10)));

    let target: HashMap<&str, Vec<Filter>> = relays
        .iter()
        .map(|url| (url.as_str(), filters.clone()))
        .collect();
    client.subscribe(target).close_on(opts).await?;

    // Sync the filters concurrently: each reconciles against every relay
    // either way, and a relay without NEG-XX support otherwise serializes
    // its initial timeout behind every other filter.
    let sync_opts = SyncOptions::default().initial_timeout(Duration::from_secs(5));
    let syncs = filters.into_iter().map(|filter| {
        let client = &client;
        let relays = &relays;
        let sync_opts = sync_opts.clone();
        async move {
            if let Err(e) = client.sync(filter).with(relays.iter()).opts(sync_opts).await {
                log::warn!("repo relay negentropy sync failed: {e}");
            }
        }
    });
    futures::future::join_all(syncs).await;

    Ok(())
}

/// Subscribe only on the bootstrap relays, auto-closing after EOSE or a
/// short timeout. Use for one-shot data fetches (repo events, profiles)
/// instead of persistent gossip-routed subscriptions.
pub(crate) async fn subscribe_bootstrap_only(
    client: &Client,
    filters: Vec<Filter>,
) -> Result<(), Error> {
    let opts = SubscribeAutoCloseOptions::default()
        .exit_policy(ReqExitPolicy::ExitOnEOSE)
        .timeout(Some(Duration::from_secs(10)));

    let target: HashMap<&str, Vec<Filter>> = BOOTSTRAP_RELAYS
        .iter()
        .map(|relay| (*relay, filters.clone()))
        .collect();

    client.subscribe(target).close_on(opts).await?;

    Ok(())
}

/// Negentropy-sync the filter against the bootstrap relays only.
pub(crate) async fn sync_bootstrap_only(
    client: &Client,
    filter: Filter,
    opts: SyncOptions,
) -> Result<SyncSummary, Error> {
    let output = client
        .sync(filter)
        .with(BOOTSTRAP_RELAYS)
        .opts(opts)
        .await?;
    Ok(output.value)
}

/// Embed a NIP-46 session key into a bunker URI as `?master=<nsec>`.
fn with_master_key(uri: &str, keys: &Keys) -> String {
    let separator = if uri.contains('?') { '&' } else { '?' };
    let nsec = keys.secret_key().to_bech32().expect("infallible");
    format!("{uri}{separator}master={nsec}")
}

/// A `https://<host>` (or `http://<host>` for `ws://` grasp servers, like
/// ngit) base URL for a grasp server. The repository then lives at
/// `{base}/{npub}/{repo-id}.git`.
fn grasp_base_url(relay: &RelayUrl) -> Option<String> {
    // `domain()` drops the port; parse the full URL to keep it (local dev
    // grasp servers commonly run on a custom port).
    let parsed = Url::parse(relay.as_str()).ok()?;
    let host = parsed.host_str()?;
    let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
    // `ws://` grasp servers (e.g. local dev relays) speak plain HTTP;
    // everything else is HTTPS, matching ngit.
    let scheme = if relay.scheme().is_secure() {
        "https"
    } else {
        "http"
    };
    Some(format!("{scheme}://{host}{port}"))
}

/// The GRASP clone URL of a repository on a grasp server, matching the
/// format ngit announces: `https://<host>/<npub>/<repo-id>.git`.
fn grasp_clone_url(relay: &RelayUrl, owner: &str, repo_id: &str) -> Option<Url> {
    let base = grasp_base_url(relay)?;
    Url::parse(&format!("{base}/{owner}/{repo_id}.git")).ok()
}

/// Push the repository at `path` to every grasp server: a server that
/// rejects the push is logged, but the push only fails when no server
/// accepted it. `push` performs the single-server push (e.g.
/// [`signed_git::push_main`] for the create flow, [`signed_git::push_all`]
/// for the init flow).
async fn push_to_grasp_servers(
    path: PathBuf,
    owner: String,
    repo_id: String,
    servers: Vec<RelayUrl>,
    push: fn(&Path, &str, &str, &str) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut failures = Vec::new();
    let mut pushed = 0;

    for relay in &servers {
        let Some(base_url) = grasp_base_url(relay) else {
            failures.push(format!("{relay}: no domain"));
            continue;
        };
        match push(&path, &base_url, &owner, &repo_id) {
            Ok(()) => pushed += 1,
            Err(e) => failures.push(format!("{relay}: {e}")),
        }
    }

    if pushed == 0 {
        bail!(
            "could not push the repository to any grasp server: {}",
            failures.join("; ")
        );
    }

    for failure in failures {
        log::warn!("grasp push failed: {failure}");
    }

    Ok(())
}

/// Split a stored bunker credential into the plain URI and the session key.
/// Credentials without an embedded key (legacy) get a fresh one.
fn extract_master_key(credential: &str) -> (&str, Keys) {
    match credential.split_once("master=") {
        Some((base, nsec)) => {
            let keys = SecretKey::parse(nsec)
                .map(Keys::new)
                .unwrap_or_else(|_| Keys::generate());
            (base.trim_end_matches(['?', '&']), keys)
        }
        None => (credential, Keys::generate()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grasp_base_url_maps_schemes_like_ngit() {
        let wss = RelayUrl::parse("wss://relay.ngit.dev").expect("url");
        assert_eq!(
            grasp_base_url(&wss).as_deref(),
            Some("https://relay.ngit.dev")
        );

        let ws = RelayUrl::parse("ws://localhost:8080").expect("url");
        assert_eq!(
            grasp_base_url(&ws).as_deref(),
            Some("http://localhost:8080")
        );
    }

    #[test]
    fn grasp_clone_url_matches_ngit_format() {
        let relay = RelayUrl::parse("wss://gitnostr.com").expect("url");
        let url = grasp_clone_url(&relay, "npub1test", "my-repo").expect("url");
        assert_eq!(
            url.to_string(),
            "https://gitnostr.com/npub1test/my-repo.git"
        );
    }
}
