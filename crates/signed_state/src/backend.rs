use std::collections::{HashMap, HashSet};
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
use signed_core::{Announcement, RepoAddr, build_state, filters, identifier_from_name};
use signed_nostr::{SignedAuthUrlHandler, UniversalSigner, Update};

use crate::git_store::GitStore;
use crate::inbox::Inbox;
use crate::repos::RepoListStore;

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

/// Relays used to index the user's NIP-65 relay list.
pub const INDEXER_RELAYS: [&str; 2] = ["wss://indexer.coracle.social", "wss://user.kindpag.es"];

const PUMP_DEBOUNCE: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
pub enum BackendEvent {
    SignerRequired,
    /// The stored identity is NIP-49 encrypted key.
    PassphraseRequired,
    /// The signer changed on login, logout or account switch.
    SignerChanged,
    /// New events were received from a relay and stored in the database.
    ///
    /// Batched: [`Backend`]'s notification pump coalesces everything a
    /// relay delivers within one debounce window into a single event,
    /// instead of emitting per-event and making every subscriber debounce
    /// the same burst independently.
    NostrUpdate(Vec<Update>),
    Synced,
    SyncProgress {
        total: u64,
        current: u64,
    },
    /// An event built locally was signed, broadcast and stored.
    Published(Box<Event>),
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

pub struct Backend {
    client: Client,
    signer: UniversalSigner,
    current_user: Option<PublicKey>,
    inbox: Entity<Inbox>,
    sync_progress: Option<(u64, u64)>,
    /// True when the stored credential is NIP-49 encrypted.
    passphrase_required: bool,
    pushing_repos: Entity<HashSet<RepoAddr>>,
}

struct GlobalBackend(Entity<Backend>);

impl Global for GlobalBackend {}

impl EventEmitter<BackendEvent> for Backend {}

impl Backend {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalBackend>().0.clone()
    }

    pub(crate) fn set_global(entity: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalBackend(entity));
    }

    pub(crate) fn new(client: Client, signer: UniversalSigner, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        let pump_client = client.clone();

        let pump: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            let mut notifications = pump_client.notifications();
            let mut pending: Vec<Update> = Vec::new();

            'outer: loop {
                match notifications.next().await {
                    Some(ClientNotification::Event { event, .. }) => {
                        pending.push(Update::from_event(&event));
                    }
                    Some(_) => continue,
                    None => break,
                }

                let deadline = Instant::now() + PUMP_DEBOUNCE;

                loop {
                    let now = Instant::now();

                    if now >= deadline {
                        break;
                    }

                    let timer = cx.background_executor().timer(deadline - now);
                    futures::pin_mut!(timer);

                    let next = notifications.next();
                    futures::pin_mut!(next);

                    match futures::future::select(next, timer).await {
                        futures::future::Either::Left((
                            Some(ClientNotification::Event { event, .. }),
                            _,
                        )) => {
                            pending.push(Update::from_event(&event));
                        }
                        futures::future::Either::Left((Some(_), _)) => continue,
                        futures::future::Either::Left((None, _)) => break 'outer,
                        futures::future::Either::Right(_) => break,
                    }
                }

                let batch = std::mem::take(&mut pending);

                if let Err(e) =
                    this.update(cx, |_this, cx| cx.emit(BackendEvent::NostrUpdate(batch)))
                {
                    log::warn!("failed to emit nostr update: {e}");
                }
            }

            Ok(())
        });

        pump.detach();

        cx.defer(move |cx| {
            if let Err(error) = weak.update(cx, |this, cx| this.bootstrap(cx)) {
                log::warn!("backend dropped before bootstrap could run: {error}");
            }
        });

        Self {
            client,
            signer,
            current_user: None,
            inbox: cx.new(|_| Inbox::default()),
            sync_progress: None,
            passphrase_required: false,
            pushing_repos: cx.new(|_| HashSet::new()),
        }
    }

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

        let notify_task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            match task.await {
                Ok(()) => {
                    this.update(cx, |this, cx| {
                        this.restore_session(cx);
                    })?;
                }
                Err(e) => {
                    this.update(cx, |_this, cx| cx.emit(BackendEvent::error(e.to_string())))?;
                }
            }
            Ok::<(), Error>(())
        });
        notify_task.detach();
    }

    /// Restore the saved session from the Keyring.
    ///
    /// - Emits [`BackendEvent::SignerRequired`] when no credential is stored.
    /// - Emits [`BackendEvent::PassphraseRequired`] for a NIP-49 encrypted identity.
    pub fn restore_session(&mut self, cx: &mut Context<Self>) {
        if cfg!(target_arch = "wasm32") {
            cx.emit(BackendEvent::SignerRequired);
            return;
        }

        let user = cx.read_credentials(USER_KEYRING);

        let task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            let content = match user.await {
                Ok(Some((_username, secret))) => String::from_utf8(secret)?,
                _ => {
                    this.update(cx, |_this, cx| cx.emit(BackendEvent::SignerRequired))?;
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
                    this.update(cx, |this, cx| {
                        this.passphrase_required = true;
                        cx.emit(BackendEvent::PassphraseRequired);
                    })?;
                } else {
                    this.update(cx, |_this, cx| cx.emit(BackendEvent::SignerRequired))?;
                }

                Ok::<_, Error>(())
            }
            .await;

            if let Err(e) = result {
                this.update(cx, |_this, cx| {
                    cx.emit(BackendEvent::error(e.to_string()));
                    cx.emit(BackendEvent::SignerRequired);
                })?;
            }

            Ok(())
        });
        task.detach();
    }

    /// Decrypt the NIP-49 keyring credential with the given passphrase.
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

            let write = cx.update(|cx| {
                cx.write_credentials(USER_KEYRING, &public_key.to_hex(), ncryptsec.as_bytes())
            });
            write.await?;

            this.update(cx, |this, cx| {
                // Become the new identity so later publishes are signed with the new keys.
                this.signer.swap_inner(keys);
                this.current_user = Some(public_key);
                this.bootstrap_user(public_key, cx);

                cx.emit(BackendEvent::SignerChanged);
                this.sync_inbox(cx);

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

                let metadata = Metadata::new()
                    .name(&name)
                    .display_name(&name)
                    .into_event_builder();

                let grasp_servers: Vec<RelayUrl> = ["wss://gitnostr.com", "wss://relay.ngit.dev"]
                    .into_iter()
                    .map(|url| RelayUrl::parse(url).expect("valid relay URL"))
                    .collect();

                let client = this.client.clone();
                let signer = this.signer.clone();

                for builder in [
                    RelayList::new(relays).into_event_builder(),
                    metadata,
                    GitUserGraspList { grasp_servers }.into_event_builder(),
                ] {
                    let client = client.clone();
                    let signer = signer.clone();
                    cx.spawn(async move |_this, _cx| {
                        publish_best_effort(&client, &signer, builder).await
                    })
                    .detach();
                }
            })?;

            Ok(public_key)
        })
    }

    /// Initialize a local clone with a `main` branch and a `README.md`.
    ///
    /// The task yields the announcement and the path of the working copy.
    pub fn create_repository(
        &mut self,
        name: &str,
        description: &str,
        folder: PathBuf,
        grasp_servers: Vec<RelayUrl>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(Announcement, PathBuf), Error>> {
        let name = name.trim().to_owned();
        let description = description.trim().to_owned();

        if name.is_empty() {
            return Task::ready(Err(anyhow!("Repository name is required")));
        }

        if grasp_servers.is_empty() {
            return Task::ready(Err(anyhow!("Add at least one grasp server")));
        }

        if !folder.is_dir() {
            return Task::ready(Err(anyhow!("Choose a folder for the repository")));
        }

        let Some(public_key) = self.current_user else {
            return Task::ready(Err(anyhow!("Sign in to create a repository")));
        };

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
        let client = self.client.clone();

        // Initialize directly at the user's chosen destination.
        // No mirror is pre-populated: `GitCache::ensure_clone` lazily clones
        // from the grasp server the first time the repo detail view needs it,
        // exactly like every other repository.
        let destination = {
            let dir_name = signed_git::sanitize_path_component(&name);
            let dir_name = if dir_name.is_empty() {
                "repository".to_owned()
            } else {
                dir_name
            };
            folder.join(dir_name)
        };

        cx.spawn(async move |this, cx| {
            let work = cx.background_spawn({
                let destination = destination.clone();
                let name = name.clone();
                let description = description.clone();
                let owner = owner.clone();
                let repo_id = repo_id.clone();
                let servers = servers.clone();

                async move {
                    if destination.exists() {
                        bail!("destination {} already exists", destination.display());
                    }

                    let commit = signed_git::init_repository(&destination, &name, &description)?;

                    if let Some(base) = servers.first().and_then(grasp_base_url) {
                        let url = format!("{base}/{owner}/{repo_id}.git");
                        signed_git::set_origin(&destination, &url)?;
                    }

                    Ok::<_, Error>(commit)
                }
            });

            let commit = work.await?;
            let commit_sha = Sha1Hash::from_str(&commit).map_err(|_| anyhow!("invalid id"))?;

            // The nostr client queues events until each relay is connected.
            for url in &servers {
                client.add_relay(url).and_connect().await.ok();
            }

            // The state event is the push authorization. It must be accepted before the push below.
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

            let signer = this.update(cx, |this, _cx| this.signer.clone())?;

            let event = {
                let builder = announcement.into_event_builder();
                let event = builder.finalize_async(&signer).await?;
                let output = client.send_event(&event).broadcast().await?;
                let event = require_relay_accepted(output, event)?;
                this.update(cx, |this, cx| this.announce_published(event.clone(), cx))?;
                event
            };

            // The state event is the push authorization. Stage it on each
            // grasp server's relay, then push the initial commit.
            // Creation fails only when no server accepted the push, the announcement
            // is then retracted so the repository is not left announced without content.
            let refs = vec![("refs/heads/main".to_owned(), commit)];

            let push = cx.background_spawn({
                let client = client.clone();
                let signer = signer.clone();
                let destination = destination.clone();
                let owner = owner.clone();
                let repo_id = repo_id.clone();
                let servers = servers.clone();
                let refs = refs.clone();
                async move {
                    push_staged_to_grasps(
                        &client,
                        &signer,
                        &repo_id,
                        &refs,
                        Some("main"),
                        &destination,
                        &owner,
                        &servers,
                        signed_git::push_main,
                    )
                    .await
                }
            });

            let outcome = push.await;

            if outcome.accepted() == 0 {
                // The announcement is already published.
                // Retract it so the repository is not left announced without content.
                this.update(cx, |this, cx| {
                    this.retract_events(std::slice::from_ref(&event), cx);
                })
                .ok();

                return Err(anyhow!(
                    "The repository was announced, but the push to every grasp server failed: {}. \
                     The announcement has been retracted",
                    outcome.failure_summary()
                ));
            }

            // Fan the state out to the relays once a git server holds the objects.
            // Staging already stored the event locally, publishing makes it
            // visible to the other relays and clients.
            if let Some(state_event) = &outcome.state_event {
                if let Err(e) = client.send_event(state_event).broadcast().await {
                    log::warn!("failed to broadcast repository state: {e}");
                }
                this.update(cx, |this, cx| {
                    this.announce_published(state_event.clone(), cx)
                })
                .ok();
            }

            let announcement = Announcement::from_event(&event)
                .ok_or_else(|| anyhow!("failed to parse announcement"))?;

            Ok((announcement, destination))
        })
    }

    /// Publish an existing local repository to NIP-34.
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
        let client = self.client.clone();

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
            for url in &servers {
                client.add_relay(url).and_connect().await.ok();
            }

            // The state event is the push authorization. It must be accepted before the push below.
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

            let signer = this.update(cx, |this, _cx| this.signer.clone())?;

            let event = {
                let builder = announcement.into_event_builder();
                let event = builder.finalize_async(&signer).await?;
                let output = client.send_event(&event).broadcast().await?;
                let event = require_relay_accepted(output, event)?;
                this.update(cx, |this, cx| this.announce_published(event.clone(), cx))?;
                event
            };

            let refs = state.refs.clone();
            let head = state.head.clone();

            // The state event is the push authorization. Stage it on each
            // grasp server's relay, then push every branch and tag. The push
            // fails only when no server accepted it. The announcement is then
            // retracted so the repository is not left announced without content.
            // An empty repository has no state to stage and nothing to push.
            if !refs.is_empty() {
                let push = cx.background_spawn({
                    let client = client.clone();
                    let signer = signer.clone();
                    let path = path.clone();
                    let owner = owner.clone();
                    let repo_id = repo_id.clone();
                    let servers = servers.clone();
                    let refs = refs.clone();
                    let head = head.clone();
                    async move {
                        push_staged_to_grasps(
                            &client,
                            &signer,
                            &repo_id,
                            &refs,
                            head.as_deref(),
                            &path,
                            &owner,
                            &servers,
                            signed_git::push_all,
                        )
                        .await
                    }
                });
                let outcome = push.await;

                if outcome.accepted() == 0 {
                    // The announcement is already published. Retract it so
                    // the repository is not left announced without content.
                    this.update(cx, |this, cx| {
                        this.retract_events(std::slice::from_ref(&event), cx);
                    })
                    .ok();

                    return Err(anyhow!(
                        "The repository was announced, but the push to every grasp server failed: {}. \
                         The announcement has been retracted",
                        outcome.failure_summary()
                    ));
                }

                // Fan the state out to the relays once a git server holds the objects.
                // Staging already stored the event locally, publishing makes it visible to the other relays and clients.
                if let Some(state_event) = &outcome.state_event {
                    if let Err(e) = client.send_event(state_event).broadcast().await {
                        log::warn!("failed to broadcast repository state: {e}");
                    }
                    this.update(cx, |this, cx| this.announce_published(state_event.clone(), cx))
                        .ok();
                }
            }

            // Point `origin` at the first grasp server so later pushes have a target.
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

    /// Re-push the repository's current refs to the grasp servers in its `relays` tag.
    ///
    /// Errors when no grasp server accepted the push, the outcome reports
    /// which servers did when only some accepted it.
    pub fn push_repository(
        &mut self,
        announcement: Announcement,
        cx: &mut Context<Self>,
    ) -> Task<Result<PushOutcome, Error>> {
        let cache = GitStore::global(cx).cache().clone();
        let path = cache.repo_path(&announcement.addr());
        self.push_repo_from(announcement, path, None, cx)
    }

    /// Push the refs of a local checkout to the grasp servers in its `relays` tag.
    /// The checkout is the working copy of the user's own repository.
    ///
    /// Publish a fresh state event, then push every branch and tag of the checkout.
    pub fn push_checkout(
        &mut self,
        announcement: Announcement,
        checkout: PathBuf,
        announced_head: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<PushOutcome, Error>> {
        self.push_repo_from(announcement, checkout, announced_head, cx)
    }

    /// Shared body of the mirror-based and checkout-based pushes.
    fn push_repo_from(
        &mut self,
        announcement: Announcement,
        path: PathBuf,
        announced_head: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<PushOutcome, Error>> {
        let addr = announcement.addr();

        if self.pushing_repos.read(cx).contains(&addr) {
            return Task::ready(Err(anyhow!(
                "A push to this repository is already in progress"
            )));
        }

        self.pushing_repos.update(cx, |pushing, cx| {
            pushing.insert(addr.clone());
            cx.notify();
        });

        let owner = announcement.owner.to_bech32().unwrap();
        let repo_id = announcement.id.clone();
        let relays = announcement.relays.clone();

        cx.spawn(async move |this, cx| {
            // Held for the whole task. Runs on completion, on error and on
            // cancellation alike, since dropping the task drops this guard.
            let _guard = cx.on_drop(&this, {
                let addr = addr.clone();
                move |backend, cx| {
                    backend.pushing_repos.update(cx, |pushing, cx| {
                        pushing.remove(&addr);
                        cx.notify();
                    });
                }
            });

            let mut state = {
                let work = cx.background_spawn({
                    let path = path.clone();
                    async move { signed_git::worktree_ref_state(&path) }
                });
                work.await?
            };

            // The state event announces the pushed refs.
            // Keep the announced default branch in `HEAD` when it is among the pushed refs.
            //
            // Otherwise `HEAD` stays the checkout's current branch.
            let heads: Vec<&str> = state
                .refs
                .iter()
                .filter_map(|(name, _)| name.strip_prefix("refs/heads/"))
                .collect();

            if let Some(head) = announced_head
                && heads.iter().any(|branch| *branch == head)
            {
                state.head = Some(head);
            }

            // Grasp servers authorize a push by the state event they hold in purgatory.
            // Stage the state event on each server's own relay, then push the git data,
            // retrying transient purgatory denials.
            let refs = state.refs.clone();
            let head = state.head.clone();

            let (client, signer) =
                this.update(cx, |this, _cx| (this.client.clone(), this.signer.clone()))?;

            let outcome = if refs.is_empty() {
                PushOutcome::default()
            } else {
                let push = cx.background_spawn({
                    let client = client.clone();
                    let signer = signer.clone();
                    let path = path.clone();
                    let owner = owner.clone();
                    let repo_id = repo_id.clone();
                    let relays = relays.clone();
                    let refs = refs.clone();
                    let head = head.clone();
                    async move {
                        push_staged_to_grasps(
                            &client,
                            &signer,
                            &repo_id,
                            &refs,
                            head.as_deref(),
                            &path,
                            &owner,
                            &relays,
                            signed_git::push_all,
                        )
                        .await
                    }
                });
                push.await
            };

            if !refs.is_empty() && outcome.accepted() == 0 {
                bail!(
                    "could not push the repository to any grasp server: {}",
                    outcome.failure_summary()
                );
            }

            // Fan the state out to the relays once a git server holds the objects.
            // Staging already stored the event locally, publishing notifies
            // the repository views and other relays and clients.
            if let Some(state_event) = &outcome.state_event {
                if let Err(e) = client.send_event(state_event).broadcast().await {
                    log::warn!("failed to broadcast repository state: {e}");
                }
                this.update(cx, |this, cx| {
                    this.announce_published(state_event.clone(), cx)
                })
                .ok();
            }

            Ok(outcome)
        })
    }

    /// Delete the repository from nostr.
    ///
    /// Only the repository owner may delete it.
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

    /// Login with an `nsec1...` key or a `bunker://...` URI.
    pub fn login(&mut self, credential: &str, cx: &mut Context<Self>) {
        let credential = credential.trim();

        if credential.starts_with("nsec1") {
            self.login_with_nsec(credential, cx);
        } else if credential.starts_with("bunker://") {
            self.login_with_bunker(credential, cx);
        } else {
            cx.emit(BackendEvent::error("Unsupported credential."));
        }
    }

    pub fn login_with_new_identity(&mut self, cx: &mut Context<Self>) {
        let nsec = Keys::generate()
            .secret_key()
            .to_bech32()
            .expect("infallible");
        self.login_with_nsec(&nsec, cx);
    }

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

        let task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            if let Err(e) = write.await {
                this.update(cx, |_this, cx| cx.emit(BackendEvent::error(e.to_string())))?;
                return Ok(());
            }
            this.update(cx, |this, cx| this.set_signer(keys, cx))?;
            Ok(())
        });
        task.detach();
    }

    /// Login with a `bunker://...` URI, NIP-46.
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

        let task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
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
                this.update(cx, |_this, cx| cx.emit(BackendEvent::error(e.to_string())))?;
            }

            Ok(())
        });
        task.detach();
    }

    pub fn logout(&mut self, cx: &mut Context<Self>) {
        let delete = cx.delete_credentials(USER_KEYRING);

        let task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            delete.await.ok();

            this.update(cx, |this, cx| {
                this.signer.swap_inner(Keys::generate());
                this.current_user = None;
                this.passphrase_required = false;
                cx.emit(BackendEvent::SignerChanged);
                cx.emit(BackendEvent::SignerRequired);
                this.sync_inbox(cx);
                cx.notify();
            })?;

            Ok(())
        });
        task.detach();
    }

    fn bootstrap_user(&mut self, public_key: PublicKey, cx: &mut Context<Self>) {
        let client = self.client.clone();

        let task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            let result = async {
                sync_bootstrap_only(
                    &client,
                    filters::grasp_list(public_key),
                    SyncOptions::default(),
                )
                .await?;

                for url in user_grasp_list_servers(client.clone(), public_key).await? {
                    client.add_relay(url).and_connect().await.ok();
                }

                Ok::<_, Error>(())
            }
            .await;

            if let Err(e) = result {
                this.update(cx, |_this, cx| cx.emit(BackendEvent::error(e.to_string())))?;
            }

            Ok(())
        });
        task.detach();
    }

    pub fn client(&self) -> Client {
        self.client.clone()
    }

    pub fn signer(&self) -> UniversalSigner {
        self.signer.clone()
    }

    pub fn pushing_repos(&self) -> Entity<HashSet<RepoAddr>> {
        self.pushing_repos.clone()
    }

    pub fn inbox(&self) -> Entity<Inbox> {
        self.inbox.clone()
    }

    pub fn current_user(&self) -> Option<PublicKey> {
        self.current_user
    }

    pub fn passphrase_required(&self) -> bool {
        self.passphrase_required
    }

    pub fn emit_error(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        cx.emit(BackendEvent::error(message));
    }

    fn sync_inbox(&mut self, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let me = self.current_user;

        if let Some(me) = me {
            self.subscribe_bootstrap(filters::notifications(me), cx);
            self.subscribe_bootstrap(vec![filters::authored_activity(me)], cx);

            let relays: HashSet<RelayUrl> = RepoListStore::global(cx)
                .read(cx)
                .announcements_of(&me)
                .into_iter()
                .flat_map(|announcement| announcement.relays)
                .collect();

            if !relays.is_empty() {
                let relays: Vec<RelayUrl> = relays.into_iter().collect();
                self.connect_repo_relays(relays.clone(), filters::notifications(me), cx);
                self.connect_repo_relays(relays, vec![filters::authored_activity(me)], cx);
            }
        }

        self.inbox.update(cx, |inbox, cx| match me {
            Some(me) => inbox.activate(me, client, cx),
            None => inbox.reset(cx),
        });
    }

    pub fn sync_progress(&self) -> Option<(u64, u64)> {
        self.sync_progress
    }

    pub fn set_signer<T>(&mut self, new_signer: T, cx: &mut Context<Self>)
    where
        T: AsyncGetPublicKey + AsyncSignEvent + AsyncNip44 + 'static,
        <T as AsyncGetPublicKey>::Error: std::error::Error + Send + Sync + 'static,
        <T as AsyncSignEvent>::Error: std::error::Error + Send + Sync + 'static,
        <T as AsyncNip44>::Error: std::error::Error + Send + Sync + 'static,
    {
        cx.spawn(async move |this, cx| {
            match new_signer.get_public_key_async().await {
                Ok(public_key) => {
                    this.update(cx, |this, cx| {
                        this.signer.swap_inner(new_signer);
                        this.current_user = Some(public_key);
                        this.passphrase_required = false;
                        this.bootstrap_user(public_key, cx);
                        cx.emit(BackendEvent::SignerChanged);
                        this.sync_inbox(cx);
                        cx.notify();
                    })?;
                }
                Err(e) => {
                    this.update(cx, |_this, cx| {
                        cx.emit(BackendEvent::error(e.to_string()));
                    })?;
                }
            }

            Ok::<(), Error>(())
        })
        .detach();
    }

    /// Connect to a repository's announced relays, its NIP-34 `relays` tag.
    pub fn connect_repo_relays(
        &mut self,
        relays: Vec<RelayUrl>,
        filters: Vec<Filter>,
        cx: &mut Context<Self>,
    ) {
        let client = self.client.clone();

        cx.spawn(async move |_this, _cx| {
            if let Err(e) = connect_repo_relays(&client, relays, filters).await {
                log::warn!("repo relay fetch failed: {e}");
            }
            Ok::<(), Error>(())
        })
        .detach();
    }

    pub fn subscribe_bootstrap(&mut self, filters: Vec<Filter>, cx: &mut Context<Self>) {
        let client = self.client.clone();

        let fetch =
            cx.background_spawn(async move { subscribe_bootstrap_only(&client, filters).await });

        cx.spawn(async move |this, cx| {
            if let Err(e) = fetch.await {
                this.update(cx, |_this, cx| {
                    cx.emit(BackendEvent::error(e.to_string()));
                })?;
            }
            Ok::<(), Error>(())
        })
        .detach();
    }

    pub fn sync_bootstrap(&mut self, filter: Filter, cx: &mut Context<Self>) {
        let client = self.client.clone();
        let (tx, mut rx) = SyncProgress::channel();

        self.sync_progress = Some((0, 0));
        cx.notify();

        let progress_task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
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
        });
        progress_task.detach();

        let sync = cx.background_spawn(async move {
            let opts = SyncOptions::default().progress(tx);
            sync_bootstrap_only(&client, filter, opts).await
        });

        let task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            match sync.await {
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
                        cx.emit(BackendEvent::error(e.to_string()))
                    })?;
                }
            }
            Ok(())
        });
        task.detach();
    }

    /// Emit [`BackendEvent::Published`] for cross-store invalidation.
    ///
    /// Callers publish with `client.send_event(...)` directly, then call this
    /// so stores like `RepoListStore` refresh without re-querying the relays.
    pub fn announce_published(&self, event: Event, cx: &mut Context<Self>) {
        cx.emit(BackendEvent::Published(Box::new(event)));
    }

    /// Publish a NIP-09 deletion for each of `events`, best-effort.
    ///
    /// Each target gets its own deletion event: a relay rejecting or
    /// dropping one does not affect the others.
    fn retract_events(&mut self, events: &[Event], cx: &mut Context<Self>) {
        let client = self.client.clone();
        let signer = self.signer.clone();

        for event in events.iter().cloned() {
            let client = client.clone();
            let signer = signer.clone();

            cx.spawn(async move |_this, _cx| {
                if let Err(e) = retract_event(&client, &signer, &event).await {
                    log::warn!("failed to retract event {}: {e}", event.id);
                }
            })
            .detach();
        }
    }
}

/// Sign and send a single NIP-09 deletion request for `event`.
async fn retract_event(
    client: &Client,
    signer: &UniversalSigner,
    event: &Event,
) -> Result<(), Error> {
    let builder = EventDeletionRequest::new()
        .id(event.id)
        .into_event_builder();
    let deletion = builder.finalize_async(signer).await?;
    client.send_event(&deletion).broadcast().await?;
    Ok(())
}

/// The event was accepted by at least one relay, or a descriptive error otherwise.
///
/// The SDK does not treat "accepted by zero relays" as an error on its own:
/// [`SendEventOutput::success`] may be empty while the call still returns `Ok`.
/// This turns that case into an error the caller can surface.
pub(crate) fn require_relay_accepted(
    output: SendEventOutput,
    event: Event,
) -> Result<Event, Error> {
    if output.success.is_empty() && !output.failed.is_empty() {
        let reasons = output
            .failed
            .values()
            .cloned()
            .collect::<Vec<String>>()
            .join(", ");
        bail!("event not accepted by any relay: {reasons}");
    }

    Ok(event)
}

/// Sign and broadcast `builder`, logging rather than surfacing failures.
///
/// Used for best-effort identity bootstrap events, where a relay hiccup
/// should not block sign-up.
async fn publish_best_effort(client: &Client, signer: &UniversalSigner, builder: EventBuilder) {
    let result: Result<(), Error> = async {
        let event = builder.finalize_async(signer).await?;
        let output = client.send_event(&event).broadcast().await?;
        require_relay_accepted(output, event)?;
        Ok(())
    }
    .await;

    if let Err(e) = result {
        log::warn!("failed to publish identity bootstrap event: {e}");
    }
}

async fn connect_repo_relays(
    client: &Client,
    relays: Vec<RelayUrl>,
    filters: Vec<Filter>,
) -> Result<(), Error> {
    if relays.is_empty() || filters.is_empty() {
        return Ok(());
    }

    for url in relays.iter() {
        client.add_relay(url).and_connect().await?;
    }

    for filter in filters.into_iter() {
        if let Err(e) = client.sync(filter).with(relays.iter()).await {
            log::warn!("repo relay negentropy sync failed: {e}");
        }
    }

    Ok(())
}

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

/// Base URL of a grasp server, `https://<host>`.
///
/// `ws://` grasp servers use `http://<host>`, like ngit.
pub(crate) fn grasp_base_url(relay: &RelayUrl) -> Option<String> {
    // `domain()` drops the port.
    let parsed = Url::parse(relay.as_str()).ok()?;
    let host = parsed.host_str()?;
    let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
    // `ws://` grasp servers, e.g. local dev relays, speak plain HTTP.
    let scheme = if relay.scheme().is_secure() {
        "https"
    } else {
        "http"
    };
    Some(format!("{scheme}://{host}{port}"))
}

fn grasp_clone_url(relay: &RelayUrl, owner: &str, repo_id: &str) -> Option<Url> {
    let base = grasp_base_url(relay)?;
    Url::parse(&format!("{base}/{owner}/{repo_id}.git")).ok()
}

/// GRASP-06 contributor namespace URL of a pull request tip.
pub(crate) fn grasp06_prs_url(base_url: &str, npub: &str, repo_id: &str) -> String {
    format!("{base_url}/prs/{npub}/{repo_id}.git")
}

/// Assemble the `clone` URLs of a pull request.
///
/// The author's GRASP-06 `/prs/` URLs come first.
pub(crate) fn pr_clone_urls(prs_urls: Vec<Url>, base_clone_urls: Vec<Url>) -> Vec<Url> {
    let mut seen = std::collections::HashSet::new();
    let mut urls = Vec::new();
    for url in prs_urls.into_iter().chain(base_clone_urls) {
        if seen.insert(url.to_string()) {
            urls.push(url);
        }
    }
    urls
}

/// The `g` tag servers of one kind-10317 grasp list event, in tag order.
fn grasp_list_servers(event: &Event) -> Vec<RelayUrl> {
    event
        .tags
        .iter()
        .filter(|tag| tag.kind() == "g")
        .filter_map(|tag| tag.content())
        .filter_map(|url| RelayUrl::parse(url).ok())
        .collect()
}

/// Grasp servers of the newest kind-10317 grasp list among `events`.
fn latest_grasp_list_servers(events: Vec<Event>) -> Vec<RelayUrl> {
    events
        .into_iter()
        .max_by_key(|event| event.created_at)
        .map(|event| grasp_list_servers(&event))
        .unwrap_or_default()
}

pub async fn user_grasp_list_servers(
    client: Client,
    user: PublicKey,
) -> Result<Vec<RelayUrl>, Error> {
    let events: Vec<Event> = client
        .database()
        .query(filters::grasp_list(user))
        .await?
        .into_iter()
        .collect();
    Ok(latest_grasp_list_servers(events))
}

const GRASP_PUSH_ATTEMPTS: usize = 3;

/// Pause before re-staging a state event after a transient denial.
const GRASP_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct GraspServerResult {
    pub relay: RelayUrl,
    pub git_url: String,
    /// `None` when the server accepted the data, the reason otherwise.
    pub reason: Option<String>,
}

impl GraspServerResult {
    fn ok(relay: RelayUrl, git_url: String) -> Self {
        Self {
            relay,
            git_url,
            reason: None,
        }
    }

    fn failed(relay: RelayUrl, git_url: String, reason: impl Into<String>) -> Self {
        Self {
            relay,
            git_url,
            reason: Some(reason.into()),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PushOutcome {
    /// Per-server results, in the order the servers were listed.
    pub servers: Vec<GraspServerResult>,
    /// The newest state event a grasp relay accepted for this push, if any.
    ///
    /// Broadcast to the other relays once a git server holds the data.
    pub state_event: Option<Event>,
}

impl PushOutcome {
    pub fn accepted(&self) -> usize {
        self.servers
            .iter()
            .filter(|server| server.reason.is_none())
            .count()
    }

    fn failing(&self) -> impl Iterator<Item = &GraspServerResult> {
        self.servers.iter().filter(|server| server.reason.is_some())
    }

    pub fn failure_summary(&self) -> String {
        self.failing()
            .map(|server| {
                let reason =
                    flatten_whitespace(server.reason.as_deref().unwrap_or("unknown error"));
                format!("{}: {reason}", server.relay)
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// A warning for a push only some grasp servers accepted.
    ///
    /// `None` when every server accepted the push or nothing was pushed.
    pub fn partial_warning(&self) -> Option<String> {
        let accepted = self.accepted();
        if self.servers.is_empty() || accepted == self.servers.len() {
            return None;
        }
        Some(format!(
            "Pushed to {accepted} of {} grasp servers: {}. Republish to sync.",
            self.servers.len(),
            self.failure_summary()
        ))
    }
}

fn flatten_whitespace(text: &str) -> String {
    const MAX_CHARS: usize = 200;
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_CHARS {
        flat
    } else {
        let mut clipped: String = flat.chars().take(MAX_CHARS).collect();
        clipped.push('…');
        clipped
    }
}

/// Reasons a push attempt should be retried with a freshly staged state
/// event and a fresh git advertisement.
///
/// Two families are retried:
///
/// - **Purgatory denials**: the grasp server sends these when the state
///   event for the push has not reached its purgatory yet. Re-staging a
///   fresh event resolves them.
/// - **Stale advertisement races**: `git receive-pack` compares each ref
///   update against the value it advertised when the push started. The grasp
///   server's own background sync can move a ref in between - typically by
///   aligning the repository to a parked state event once the objects of an
///   earlier attempt land - so the compare-and-swap fails with `cannot lock
///   ref` / `incorrect old value provided`. A retry against the fresh
///   advertisement converges, and when the race is lost the pushed data is
///   usually already on the server (see `is_stale_advertisement_race` and
///   the convergence probe in `push_staged_to_grasps`).
///
/// Other rejections are not retried.
fn is_transient_grasp_denial(stderr: &str) -> bool {
    let error = stderr.to_lowercase();
    [
        "no state events in purgatory",
        "no matching state event",
        "doesn't match push",
        "none from authorized publishers",
        "no repository announcement found",
        "cannot lock ref",
        "incorrect old value provided",
    ]
    .iter()
    .any(|marker| error.contains(marker))
}

/// A push rejected because `git receive-pack`'s compare-and-swap lost to the
/// grasp server's own background ref alignment: the ref moved between this
/// push's advertisement and its ref transaction (`cannot lock ref ... is at
/// ... but expected ...` / `incorrect old value provided`). The pushed data
/// is usually already on the server by then.
fn is_stale_advertisement_race(stderr: &str) -> bool {
    let error = stderr.to_lowercase();
    error.contains("cannot lock ref") || error.contains("incorrect old value provided")
}

/// Keep `event` as the push's fan-out state event when it is newer than the
/// current one. All staged events carry the same refs; the newest timestamp
/// wins on the relays.
fn keep_newest(state_event: &mut Option<Event>, event: Event) {
    if state_event
        .as_ref()
        .is_none_or(|current| event.created_at > current.created_at)
    {
        *state_event = Some(event);
    }
}

/// Sign a fresh kind `30618` state event for the push.
///
/// `last_created_at` is the timestamp of the previous event signed for this push.
/// Retries within the same second get the next second: a grasp relay
/// treats a same-id resend as a duplicate and does not re-run its ingest,
/// so an identical resend cannot re-park a state event lost from its purgatory.
async fn sign_state_event(
    signer: &UniversalSigner,
    repo_id: &str,
    refs: &[(String, String)],
    head: Option<&str>,
    last_created_at: u64,
) -> Result<(Event, u64), String> {
    let now = Timestamp::now().as_secs();
    let created_at = if now > last_created_at {
        now
    } else {
        last_created_at + 1
    };

    let event = build_state(repo_id, refs, head)
        .custom_created_at(Timestamp::from_secs(created_at))
        .finalize_async(signer)
        .await
        .map_err(|e| format!("could not sign the state event: {e}"))?;

    Ok((event, created_at))
}

/// Ensure the relay is known and connected, then publish `event` to it.
///
/// `Ok` only when the relay confirmed the event.
/// On a grasp relay the accept parks the event in purgatory,
/// which authorizes the paired git push.
async fn stage_event_on_relay(
    client: &Client,
    relay: &RelayUrl,
    event: &Event,
) -> Result<(), String> {
    client
        .add_relay(relay)
        .and_connect()
        .await
        .map_err(|e| format!("could not add relay {relay}: {e}"))?;

    let output = client
        .send_event(event)
        .to([relay.clone()])
        .await
        .map_err(|e| format!("could not send the state event to {relay}: {e}"))?;

    if output.success.contains_key(relay) {
        Ok(())
    } else {
        let reason = output
            .failed
            .get(relay)
            .cloned()
            .unwrap_or_else(|| "relay did not confirm the event".to_owned());
        Err(reason)
    }
}

#[allow(clippy::too_many_arguments)]
async fn push_staged_to_grasps(
    client: &Client,
    signer: &UniversalSigner,
    repo_id: &str,
    refs: &[(String, String)],
    head: Option<&str>,
    path: &Path,
    owner: &str,
    servers: &[RelayUrl],
    push: fn(&Path, &str, &str, &str) -> Result<(), Error>,
) -> PushOutcome {
    let mut outcome = PushOutcome::default();

    if refs.is_empty() {
        return outcome;
    }

    for relay in servers {
        let Some(base) = grasp_base_url(relay) else {
            outcome.servers.push(GraspServerResult::failed(
                relay.clone(),
                relay.to_string(),
                "no domain",
            ));
            continue;
        };
        let git_url = format!("{base}/{owner}/{repo_id}.git");

        let mut reason = None;
        let mut last_created_at = 0;
        // The last state event staged on this server, for the convergence
        // probe below when every push attempt lost the stale-ref race.
        let mut staged_event = None;

        'server: for attempt in 1..=GRASP_PUSH_ATTEMPTS {
            if attempt > 1 {
                // Give the server's ingest a moment before re-staging.
                std::thread::sleep(GRASP_RETRY_DELAY);
            }

            let (event, created_at) =
                match sign_state_event(signer, repo_id, refs, head, last_created_at).await {
                    Ok(signed) => signed,
                    Err(e) => {
                        reason = Some(e);
                        break 'server;
                    }
                };

            last_created_at = created_at;

            // Stage the state event on this server's own relay.
            // A failed stage means the grasp never parked the state,
            // so the git push would be denied anyway: skip it (the eligibility gate).
            if let Err(e) = stage_event_on_relay(client, relay, &event).await {
                // One retry absorbs a relay connect blip, on the first
                // attempt only.
                if attempt == 1 && stage_event_on_relay(client, relay, &event).await.is_ok() {
                    // staged on the retry
                } else {
                    reason = Some(e);
                    break 'server;
                }
            }
            staged_event = Some(event.clone());

            match push(path, &base, owner, repo_id) {
                Ok(()) => {
                    keep_newest(&mut outcome.state_event, event);
                    break 'server;
                }
                Err(e) => {
                    let text = e.to_string();
                    if attempt < GRASP_PUSH_ATTEMPTS && is_transient_grasp_denial(&text) {
                        reason = Some(text);
                        continue 'server;
                    }
                    reason = Some(text);
                    break 'server;
                }
            }
        }

        // The grasp's own background sync aligns refs to staged state
        // events as soon as the objects land, which can beat every push
        // attempt's compare-and-swap (`cannot lock ref ... but expected`).
        // When the last denial was that race the sync has usually finished
        // by now: verify the advertised refs and accept the server when the
        // pushed data is already there.
        if let Some(last_reason) = &reason
            && is_stale_advertisement_race(last_reason)
            && signed_git::remote_has_refs(path, &git_url, refs).unwrap_or(false)
        {
            if let Some(event) = staged_event {
                keep_newest(&mut outcome.state_event, event);
            }
            reason = None;
        }

        match reason {
            Some(reason) => {
                log::warn!("grasp push failed: {relay}: {reason}");
                outcome
                    .servers
                    .push(GraspServerResult::failed(relay.clone(), git_url, reason));
            }
            None => outcome
                .servers
                .push(GraspServerResult::ok(relay.clone(), git_url)),
        }
    }

    outcome
}

/// Split a stored bunker credential into the plain URI and the session key.
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

    #[test]
    fn grasp06_prs_url_matches_ngit_format() {
        assert_eq!(
            grasp06_prs_url("https://relay.ngit.dev", "npub1author", "my-repo"),
            "https://relay.ngit.dev/prs/npub1author/my-repo.git"
        );
        // `ws://` grasp servers, local dev, keep their plain-HTTP base.
        assert_eq!(
            grasp06_prs_url("http://localhost:8080", "npub1author", "my-repo"),
            "http://localhost:8080/prs/npub1author/my-repo.git"
        );
    }

    #[test]
    fn pr_clone_urls_orders_author_first_and_deduplicates() {
        let prs = vec![
            Url::parse("https://a.example/prs/npub1me/repo.git").expect("url"),
            Url::parse("https://a.example/prs/npub1me/repo.git").expect("url"),
        ];
        let base = vec![
            Url::parse("https://a.example/npub1owner/repo.git").expect("url"),
            Url::parse("https://b.example/npub1owner/repo.git").expect("url"),
            Url::parse("https://b.example/npub1owner/repo.git").expect("url"),
        ];

        let urls = pr_clone_urls(prs, base);
        assert_eq!(
            urls.iter().map(ToString::to_string).collect::<Vec<_>>(),
            vec![
                "https://a.example/prs/npub1me/repo.git",
                "https://a.example/npub1owner/repo.git",
                "https://b.example/npub1owner/repo.git",
            ]
        );
    }

    #[test]
    fn transient_grasp_denials_are_classified() {
        // The exact server rejection that started this work: the state event
        // had not reached the grasp's purgatory before the git push.
        let reported = "remote: ERR authorisation failed: No state events in purgatory\n\
            fatal: the remote end hung up unexpectedly\n\
            error: failed to push some refs to 'https://relay.ngit.dev/...git'";
        assert!(is_transient_grasp_denial(reported));

        // The other purgatory states a fresh event resolves.
        assert!(is_transient_grasp_denial(
            "remote: ERR authorisation failed: No matching state event found in purgatory"
        ));
        assert!(is_transient_grasp_denial(
            "remote: ERR authorisation failed: 1 state event in purgatory from authorized \
             publisher but doesn't match push"
        ));
        assert!(is_transient_grasp_denial(
            "remote: ERR authorisation failed: 2 state events in purgatory but none from \
             authorized publishers"
        ));
        assert!(is_transient_grasp_denial(
            "remote: ERR authorisation failed: No repository announcement found"
        ));

        // Rejections a fresh state event cannot fix are not retried.
        assert!(!is_transient_grasp_denial(
            "remote: ERR authorisation failed: not a maintainer of this repository"
        ));
        assert!(!is_transient_grasp_denial(
            "fatal: unable to access 'https://relay.ngit.dev/...': The requested URL returned \
             error: 403"
        ));
        assert!(!is_transient_grasp_denial(
            "fatal: unable to access 'https://relay.ngit.dev/...': Could not resolve host"
        ));
    }

    #[test]
    fn stale_ref_races_are_retried() {
        // The grasp's background sync aligned the ref to a parked state event
        // between this push's advertisement and its ref transaction. The ref
        // is usually already where the push wants it, so a retry converges.
        let reported = "remote: error: cannot lock ref 'refs/heads/main': is at \
            cac2ac91b6f5fb8dfcb6962785babc6e65350cb3 but expected \
            bc5e892aa84dc6240a5fbcd59367a4857d26f49b\n\
            To https://relay.ngit.dev/npub1owner/signed-test.git\n\
             ! [remote rejected] main -> main (incorrect old value provided)\n\
            error: failed to push some refs to 'https://relay.ngit.dev/npub1owner/signed-test.git'";
        assert!(is_transient_grasp_denial(reported));
        assert!(is_stale_advertisement_race(reported));

        // Markers match independently of the surrounding git output.
        assert!(is_stale_advertisement_race(
            "cannot lock ref 'refs/heads/main'"
        ));
        assert!(is_stale_advertisement_race(
            "! [remote rejected] main -> main (incorrect old value provided)"
        ));

        // A purgatory denial is not a stale-advertisement race.
        assert!(!is_stale_advertisement_race("No state events in purgatory"));

        // A real divergence is a different error and stays permanent.
        assert!(!is_transient_grasp_denial(
            " ! [rejected]        main -> main (non-fast-forward)"
        ));
    }

    #[test]
    fn push_outcome_reports_partial_failures() {
        let outcome = PushOutcome {
            servers: vec![
                GraspServerResult::ok(
                    RelayUrl::parse("wss://gitnostr.com").expect("url"),
                    "https://gitnostr.com/npub1owner/repo.git".to_owned(),
                ),
                GraspServerResult::failed(
                    RelayUrl::parse("wss://relay.ngit.dev").expect("url"),
                    "https://relay.ngit.dev/npub1owner/repo.git".to_owned(),
                    "remote: ERR authorisation failed: No state events in purgatory\nfatal: ...",
                ),
            ],
            state_event: None,
        };

        assert_eq!(outcome.accepted(), 1);
        assert_eq!(
            outcome.failure_summary(),
            "wss://relay.ngit.dev: remote: ERR authorisation failed: No state events in \
             purgatory fatal: ..."
        );
        let warning = outcome.partial_warning().expect("partial push warning");
        assert!(warning.starts_with("Pushed to 1 of 2 grasp servers"));
        assert!(warning.contains("Republish to sync"));
        // The multi-line server reason is a single display line.
        assert_eq!(warning.lines().count(), 1);
    }
}
