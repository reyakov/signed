use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{Duration, Instant};

use anyhow::{Error, anyhow, bail};
use bitcoin_hashes::sha1::Hash as Sha1Hash;
use gpui::{
    App, AppContext, AsyncApp, Context, Entity, EventEmitter, Global, Task, TaskExt, WeakEntity,
};
use nostr::event::IntoEventBuilder;
use nostr::nips::nip19::Nip19Coordinate;
use nostr_connect::prelude::*;
use nostr_sdk::prelude::*;
use signed_core::{Announcement, Filters, RepoAddr, filters};
use signed_git::{GitCache, Repo};
use signed_nostr::{SignedAuthUrlHandler, UniversalSigner, Update};

use crate::bootstrap::{
    ensure_bootstrap_relays, subscribe_bootstrap_only, sync_bootstrap_only, user_grasp_list_servers,
};
use crate::git_store::Mirrors;
use crate::inbox::Inbox;
use crate::push::{GraspPush, PushOutcome, grasp_base_url, grasp_clone_url};
use crate::repos::RepoListStore;

pub const USER_KEYRING: &str = "Signed Safe Storage";
pub const NOSTR_CONNECT_TIMEOUT: u64 = 60;

const PUMP_DEBOUNCE: Duration = Duration::from_millis(200);

#[derive(Debug, Clone)]
pub enum BackendEvent {
    SignerRequired,
    PassphraseRequired,
    SignerChanged,
    ProfileUpdates(Vec<PublicKey>),
    RepoUpdates(Vec<Update>),
    Synced,
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
            let mut pending_profiles: HashSet<PublicKey> = HashSet::new();
            let mut pending_grasps: HashSet<PublicKey> = HashSet::new();
            let mut pending_repos: Vec<Update> = Vec::new();
            let mut seen: HashSet<EventId> = HashSet::new();

            'outer: loop {
                match UpdateEvent::next(&mut notifications, &mut seen).await {
                    Some(UpdateEvent::Profile(author)) => {
                        pending_profiles.insert(author);
                    }
                    Some(UpdateEvent::Grasp(author)) => {
                        pending_grasps.insert(author);
                    }
                    Some(UpdateEvent::Repo(update)) => pending_repos.push(update),
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

                    let next = UpdateEvent::next(&mut notifications, &mut seen);
                    futures::pin_mut!(next);

                    match futures::future::select(next, timer).await {
                        futures::future::Either::Left((Some(UpdateEvent::Profile(author)), _)) => {
                            pending_profiles.insert(author);
                        }
                        futures::future::Either::Left((Some(UpdateEvent::Grasp(author)), _)) => {
                            pending_grasps.insert(author);
                        }
                        futures::future::Either::Left((Some(UpdateEvent::Repo(update)), _)) => {
                            pending_repos.push(update);
                        }
                        futures::future::Either::Left((None, _)) => break 'outer,
                        futures::future::Either::Right(_) => break,
                    }
                }

                let profiles: Vec<PublicKey> = pending_profiles.drain().collect();
                let grasps: Vec<PublicKey> = pending_grasps.drain().collect();
                let repos = std::mem::take(&mut pending_repos);

                this.update(cx, |this, cx| {
                    if !profiles.is_empty() {
                        cx.emit(BackendEvent::ProfileUpdates(profiles));
                    }

                    if !repos.is_empty() {
                        cx.emit(BackendEvent::RepoUpdates(repos));
                    }

                    for author in grasps {
                        if this.current_user == Some(author) {
                            this.connect_grasp_relays(author, cx);
                        }
                    }
                })
                .ok();
            }

            Ok(())
        });

        pump.detach();

        cx.defer(move |cx| {
            weak.update(cx, |this, cx| {
                this.restore_session(cx);
            })
            .ok();
        });

        Self {
            client,
            signer,
            current_user: None,
            inbox: cx.new(|_| Inbox::default()),
            passphrase_required: false,
            pushing_repos: cx.new(|_| HashSet::new()),
        }
    }

    fn restore_session(&mut self, cx: &mut Context<Self>) {
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
                    this.update(cx, |this, cx| {
                        this.set_signer(keys, cx);
                    })?;
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

                    this.update(cx, |this, cx| {
                        this.set_signer(signer, cx);
                    })?;
                } else if content.starts_with("ncryptsec1") {
                    this.update(cx, |this, cx| {
                        this.passphrase_required = true;
                        cx.emit(BackendEvent::PassphraseRequired);
                    })?;
                } else {
                    this.update(cx, |_this, cx| {
                        cx.emit(BackendEvent::SignerRequired);
                    })?;
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

    /// Import an `nsec1...` secret key, storing it NIP-49 encrypted with `password`.
    pub fn import_nsec(
        &mut self,
        nsec: &str,
        password: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<PublicKey, Error>> {
        let keys = match SecretKey::parse(nsec.trim()) {
            Ok(secret) => Keys::new(secret),
            Err(e) => return Task::ready(Err(anyhow!(e))),
        };

        if password.is_empty() {
            return Task::ready(Err(anyhow!("Passphrase must not be empty")));
        }

        let password = password.to_owned();

        cx.spawn(async move |this, cx| {
            let job = cx.background_spawn(async move {
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

            this.update(cx, |this, cx| this.set_signer(keys, cx))?;

            Ok(public_key)
        })
    }

    /// Import an NIP-49 encrypted secret key (`ncryptsec1...`), decrypting it with `password`.
    pub fn import_ncryptsec(
        &mut self,
        ncryptsec: &str,
        password: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<PublicKey, Error>> {
        let ncryptsec = ncryptsec.trim().to_owned();

        if password.is_empty() {
            return Task::ready(Err(anyhow!("Passphrase must not be empty")));
        }

        let password = password.to_owned();

        cx.spawn(async move |this, cx| {
            let stored = ncryptsec.clone();
            let decrypt_task = cx.background_spawn(async move {
                let encrypted = EncryptedSecretKey::from_bech32(&ncryptsec)?;
                let secret = encrypted.decrypt(&password)?;
                Ok::<_, Error>(Keys::new(secret))
            });

            let keys = decrypt_task.await?;
            let public_key = keys.public_key();

            let write = cx.update(|cx| {
                cx.write_credentials(USER_KEYRING, &public_key.to_hex(), stored.as_bytes())
            });
            write.await?;

            this.update(cx, |this, cx| this.set_signer(keys, cx))?;

            Ok(public_key)
        })
    }

    /// Import a `bunker://...` URI (NIP-46), connecting to the remote signer.
    pub fn import_bunker(
        &mut self,
        uri: &str,
        cx: &mut Context<Self>,
    ) -> Task<Result<PublicKey, Error>> {
        let uri_string = uri.trim().to_owned();

        let connect_uri = match NostrConnectUri::parse(&uri_string) {
            Ok(uri) => uri,
            Err(e) => return Task::ready(Err(anyhow!(e))),
        };

        let keys = Keys::generate();
        let credential = with_master_key(&uri_string, &keys);
        let write = cx.write_credentials(USER_KEYRING, "bunker", credential.as_bytes());

        cx.spawn(async move |this, cx| {
            let mut signer = NostrConnect::new(
                connect_uri,
                keys,
                Duration::from_secs(NOSTR_CONNECT_TIMEOUT),
                None,
            )?;
            signer.auth_url_handler(SignedAuthUrlHandler);

            // Verify the bunker responds before persisting the credential.
            let public_key = signer.get_public_key_async().await?;
            write.await?;

            this.update(cx, |this, cx| this.set_signer(signer, cx))?;

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

                let metadata = Metadata::new()
                    .name(&name)
                    .display_name(&name)
                    .into_event_builder();

                let grasp_servers: Vec<RelayUrl> = ["wss://gitnostr.com", "wss://relay.ngit.dev"]
                    .into_iter()
                    .map(|url| RelayUrl::parse(url).expect("valid relay URL"))
                    .collect();

                let pusher = GraspPush::new(this.client.clone(), this.signer.clone());

                for builder in [
                    RelayList::new(relays).into_event_builder(),
                    metadata,
                    GitUserGraspList { grasp_servers }.into_event_builder(),
                ] {
                    let pusher = pusher.clone();
                    cx.background_spawn(async move {
                        pusher.publish_best_effort(builder).await;
                    })
                    .detach();
                }
            })?;

            Ok(public_key)
        })
    }

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

        let repo_id = RepoAddr::identifier_from_name(&name);

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
        let signer = self.signer.clone();

        let destination = {
            let dir_name = GitCache::sanitize_path_component(&name);
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

                    let commit = Repo::init(&destination, &name, &description)?;

                    if let Some(base) = servers.first().and_then(grasp_base_url) {
                        let url = format!("{base}/{owner}/{repo_id}.git");
                        Repo::open(&destination)?.set_origin(&url)?;
                    }

                    Ok::<_, Error>(commit)
                }
            });

            let commit = work.await?;
            let commit_sha = Sha1Hash::from_str(&commit).map_err(|_| anyhow!("invalid id"))?;

            let announcement = build_announcement(
                &repo_id,
                &name,
                &description,
                &owner,
                &servers,
                Some(commit_sha),
            );

            let event = announce_repository_and_push(
                &this,
                &client,
                &signer,
                announcement,
                &repo_id,
                &owner,
                &servers,
                vec![("refs/heads/main".to_owned(), commit)],
                Some("main".to_owned()),
                &destination,
                cx,
                |path, base, owner, repo_id| Repo::open(path)?.push_main(base, owner, repo_id),
            )
            .await?;

            let announcement = Announcement::from_event(&event)
                .ok_or_else(|| anyhow!("failed to parse announcement"))?;

            Ok((announcement, destination))
        })
    }

    pub fn publish_local_repo(
        &mut self,
        path: PathBuf,
        name: &str,
        description: &str,
        grasp_servers: Vec<RelayUrl>,
        default_branch: Option<String>,
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

        let repo_id = RepoAddr::identifier_from_name(&name);

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
        let signer = self.signer.clone();

        cx.spawn(async move |this, cx| {
            let (mut state, euc) = cx
                .background_spawn({
                    let path = path.clone();
                    async move {
                        let repo = Repo::open(&path)?;
                        let state = repo.ref_state()?;
                        let euc = repo.root_commit()?;
                        Ok::<_, Error>((state, euc))
                    }
                })
                .await?;

            let chosen = default_branch
                .as_deref()
                .map(str::trim)
                .filter(|branch| !branch.is_empty());

            if let Some(branch) = chosen {
                let wanted = format!("refs/heads/{branch}");
                if state.refs.iter().any(|(name, _)| name == &wanted) {
                    state.head = Some(branch.to_owned());
                }
            }

            let euc = euc.and_then(|commit| Sha1Hash::from_str(&commit).ok());
            let ann = build_announcement(&repo_id, &name, &description, &owner, &servers, euc);

            let event = announce_repository_and_push(
                &this,
                &client,
                &signer,
                ann,
                &repo_id,
                &owner,
                &servers,
                state.refs.clone(),
                state.head.clone(),
                &path,
                cx,
                |path, base, owner, repo_id| Repo::open(path)?.push_all(base, owner, repo_id),
            )
            .await?;

            if let Some(base) = servers.first().and_then(grasp_base_url) {
                let url = format!("{base}/{owner}/{repo_id}.git");
                let path = path.clone();
                cx.background_spawn(async move {
                    if let Err(e) = Repo::open(&path).and_then(|r| r.ensure_origin(&url)) {
                        log::warn!("failed to ensure origin: {e}");
                    }
                })
                .await;
            }

            // The ngit-compatible `nostr.repo` marker makes the next scan
            // detect the repository instead of offering to publish it again.
            let coordinate = RepoAddr::new(event.pubkey, repo_id.clone());
            match Nip19Coordinate::new(coordinate.into(), servers.clone()).to_bech32() {
                Ok(naddr) => {
                    let path = path.clone();
                    cx.background_spawn(async move {
                        if let Err(e) = Repo::open(&path).and_then(|r| r.set_nostr_repo(&naddr)) {
                            log::warn!("failed to record the NIP-34 marker: {e}");
                        }
                    })
                    .await;
                }
                Err(error) => log::warn!("failed to encode the repository coordinate: {error}"),
            }

            Announcement::from_event(&event).ok_or_else(|| anyhow!("failed to parse announcement"))
        })
    }

    // Errors when no grasp server accepted the push.
    pub fn push_repository(
        &mut self,
        announcement: Announcement,
        cx: &mut Context<Self>,
    ) -> Task<Result<PushOutcome, Error>> {
        let path = Mirrors::path(&announcement.addr());
        self.push_repo_from(announcement, path, None, cx)
    }

    pub fn push_checkout(
        &mut self,
        announcement: Announcement,
        checkout: PathBuf,
        announced_head: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<PushOutcome, Error>> {
        self.push_repo_from(announcement, checkout, announced_head, cx)
    }

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
        let client = self.client.clone();
        let signer = self.signer.clone();

        cx.spawn(async move |this, cx| {
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
                    async move { Repo::open(&path).and_then(|repo| repo.ref_state()) }
                });
                work.await?
            };

            // Keep the announced default branch in `HEAD` when it is among the pushed refs.
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
            let refs = state.refs.clone();
            let head = state.head.clone();

            let outcome = if refs.is_empty() {
                PushOutcome::default()
            } else {
                let push = cx.background_spawn({
                    let pusher = GraspPush::new(client.clone(), signer.clone());
                    let path = path.clone();
                    let owner = owner.clone();
                    let repo_id = repo_id.clone();
                    let relays = relays.clone();
                    let refs = refs.clone();
                    let head = head.clone();
                    let executor = cx.background_executor().clone();
                    async move {
                        pusher
                            .push_staged_to_grasps(
                                &repo_id,
                                &refs,
                                head.as_deref(),
                                &path,
                                &owner,
                                &relays,
                                &executor,
                                |path, base, owner, repo_id| {
                                    Repo::open(path)?.push_all(base, owner, repo_id)
                                },
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
            if let Some(state_event) = &outcome.state_event
                && let Err(e) = client.send_event(state_event).broadcast().await
            {
                log::warn!("failed to broadcast repository state: {e}");
            }

            Ok(outcome)
        })
    }

    pub fn delete_repository(
        &mut self,
        addr: RepoAddr,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), Error>> {
        let Some(public_key) = self.current_user else {
            return Task::ready(Err(anyhow!("Sign in to delete a repository")));
        };
        if public_key != addr.public_key() {
            return Task::ready(Err(anyhow!("Only the repository owner can delete it")));
        }

        let client = self.client.clone();
        let addr = addr.clone();

        cx.spawn(async move |this, cx| {
            let events = cx.background_spawn(async move {
                let db = client.database();
                let mut events = Vec::new();
                for filter in [
                    addr.announcement_filter(),
                    addr.state_filter(),
                    addr.activity_filter(),
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

    fn bootstrap_user(&mut self, public_key: PublicKey, cx: &mut Context<Self>) {
        let client = self.client.clone();

        cx.spawn(async move |this, cx| {
            let result: Result<(), anyhow::Error> = cx
                .background_spawn(async move {
                    ensure_bootstrap_relays(&client).await?;

                    for filter in Filters::user_metadata(public_key) {
                        client.sync(filter).await?;
                    }

                    Ok(())
                })
                .await;

            if let Err(e) = result {
                this.update(cx, |_this, cx| {
                    cx.emit(BackendEvent::error(e.to_string()));
                })?;
            }

            Ok::<(), Error>(())
        })
        .detach();
    }

    /// Runs when the pump sees the user's grasp list event arrive.
    fn connect_grasp_relays(&mut self, public_key: PublicKey, cx: &mut Context<Self>) {
        let client = self.client.clone();

        cx.spawn(async move |this, cx| {
            match user_grasp_list_servers(&client, public_key).await {
                Ok(servers) => {
                    for url in servers {
                        if let Err(e) = client.add_relay(&url).and_connect().await {
                            log::warn!("failed to connect grasp relay {url}: {e}");
                        }
                    }
                    this.update(cx, |this, cx| {
                        this.sync_inbox(cx);
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

    pub fn client(&self) -> Client {
        self.client.clone()
    }

    pub fn signer(&self) -> UniversalSigner {
        self.signer.clone()
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

    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        self.current_user = None;
        self.passphrase_required = false;

        self.inbox.update(cx, |inbox, cx| {
            inbox.reset(cx);
        });

        cx.delete_credentials(USER_KEYRING).detach_and_log_err(cx);
        cx.emit(BackendEvent::SignerChanged);
        cx.emit(BackendEvent::SignerRequired);
        cx.notify();
    }

    fn sync_inbox(&mut self, cx: &mut Context<Self>) {
        let repo_store = RepoListStore::global(cx);
        let client = self.client.clone();
        let me = self.current_user;

        if let Some(me) = me {
            self.subscribe_bootstrap(Filters::notifications(me), cx);
            self.subscribe_bootstrap(vec![Filters::authored_activity(me)], cx);

            let relays: HashSet<RelayUrl> = repo_store
                .read(cx)
                .announcements_of(&me)
                .into_iter()
                .flat_map(|announcement| announcement.relays)
                .collect();

            if !relays.is_empty() {
                let relays: Vec<RelayUrl> = relays.into_iter().collect();
                self.connect_repo_relays(relays.clone(), Filters::notifications(me), cx);
                self.connect_repo_relays(relays, vec![Filters::authored_activity(me)], cx);
            }
        }

        self.inbox.update(cx, |inbox, cx| match me {
            Some(me) => inbox.activate(me, client, cx),
            None => inbox.reset(cx),
        });
    }

    fn set_signer<T>(&mut self, new_signer: T, cx: &mut Context<Self>)
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

    pub fn connect_repo_relays(
        &mut self,
        relays: Vec<RelayUrl>,
        filters: Vec<Filter>,
        cx: &mut Context<Self>,
    ) {
        if relays.is_empty() || filters.is_empty() {
            return;
        }

        let client = self.client.clone();

        cx.background_spawn(async move {
            let connected: Result<(), Error> = async {
                for url in relays.iter() {
                    client.add_relay(url).and_connect().await?;
                }
                Ok(())
            }
            .await;

            if let Err(e) = connected {
                log::warn!("repo relay fetch failed: {e}");
                return Ok::<(), Error>(());
            }

            for filter in filters.into_iter() {
                if let Err(e) = client.sync(filter).with(relays.iter()).await {
                    log::warn!("repo relay negentropy sync failed: {e}");
                }
            }

            Ok::<(), Error>(())
        })
        .detach();
    }

    pub fn sync_auto(&mut self, filters: Vec<Filter>, cx: &mut Context<Self>) {
        let client = self.client.clone();

        cx.spawn(async move |_this, _cx| {
            for filter in filters {
                if let Err(e) = client.sync(filter).await {
                    log::warn!("gossip relay fetch failed: {e}");
                }
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

    pub fn sync_bootstraps(&mut self, filters: Vec<Filter>, cx: &mut Context<Self>) {
        let client = self.client.clone();

        let sync = cx.background_spawn(async move {
            let mut first_error = None;

            for filter in filters {
                if let Err(error) =
                    sync_bootstrap_only(&client, filter, SyncOptions::default()).await
                {
                    first_error.get_or_insert(error);
                }
            }

            match first_error {
                Some(error) => Err(error),
                None => Ok(()),
            }
        });

        cx.spawn(async move |this, cx| {
            match sync.await {
                Ok(_) => {
                    this.update(cx, |_this, cx| {
                        cx.emit(BackendEvent::Synced);
                        cx.notify();
                    })?;
                }
                Err(e) => {
                    this.update(cx, |_this, cx| cx.emit(BackendEvent::error(e.to_string())))?;
                }
            }

            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    // Each target gets its own deletion event.
    fn retract_events(&mut self, events: &[Event], cx: &mut Context<Self>) {
        let pusher = GraspPush::new(self.client.clone(), self.signer.clone());

        for event in events.iter().cloned() {
            let pusher = pusher.clone();

            cx.spawn(async move |_this, _cx| {
                if let Err(e) = pusher.retract_event(&event).await {
                    log::warn!("failed to retract event {}: {e}", event.id);
                }
            })
            .detach();
        }
    }
}

fn build_announcement(
    repo_id: &str,
    name: &str,
    description: &str,
    owner: &str,
    servers: &[RelayUrl],
    euc: Option<Sha1Hash>,
) -> GitRepositoryAnnouncement {
    GitRepositoryAnnouncement {
        id: repo_id.to_owned(),
        name: Some(name.to_owned()),
        description: (!description.is_empty()).then(|| description.to_owned()),
        web: Vec::new(),
        clone: servers
            .iter()
            .filter_map(|relay| grasp_clone_url(relay, owner, repo_id))
            .collect(),
        relays: servers.to_vec(),
        euc,
        maintainers: Vec::new(),
    }
}

// Announce the repository, then stage the state event and push.
#[allow(clippy::too_many_arguments)]
async fn announce_repository_and_push(
    backend: &WeakEntity<Backend>,
    client: &Client,
    signer: &UniversalSigner,
    announcement: GitRepositoryAnnouncement,
    repo_id: &str,
    owner: &str,
    servers: &[RelayUrl],
    refs: Vec<(String, String)>,
    head: Option<String>,
    path: &Path,
    cx: &mut AsyncApp,
    push: impl Fn(&Path, &str, &str, &str) -> Result<(), Error> + Send + 'static,
) -> Result<Event, Error> {
    for url in servers {
        client.add_relay(url).and_connect().await.ok();
    }

    let event = {
        let builder = announcement.into_event_builder();
        let event = builder.finalize_async(signer).await?;
        let output = client.send_event(&event).broadcast().await?;
        GraspPush::require_relay_accepted(output, event)?
    };

    // The state event is the push authorization.
    let outcome = if refs.is_empty() {
        PushOutcome::default()
    } else {
        let pusher = GraspPush::new(client.clone(), signer.clone());
        let repo_id = repo_id.to_owned();
        let owner = owner.to_owned();
        let servers = servers.to_vec();
        let head = head.clone();
        let path = path.to_path_buf();
        let executor = cx.background_executor().clone();
        cx.background_spawn(async move {
            pusher
                .push_staged_to_grasps(
                    &repo_id,
                    &refs,
                    head.as_deref(),
                    &path,
                    &owner,
                    &servers,
                    &executor,
                    push,
                )
                .await
        })
        .await
    };

    if outcome.accepted() == 0 {
        // Retract the announcement so the repository is not left announced without content.
        backend
            .update(cx, |backend, cx| {
                backend.retract_events(std::slice::from_ref(&event), cx);
            })
            .ok();

        return Err(anyhow!(
            "The repository was announced, but the push to every grasp server failed: {}. \
             The announcement has been retracted",
            outcome.failure_summary()
        ));
    }

    // Fan the state out to the relays once a git server holds the objects.
    if let Some(state_event) = &outcome.state_event
        && let Err(e) = client.send_event(state_event).broadcast().await
    {
        log::warn!("failed to broadcast repository state: {e}");
    }

    Ok(event)
}

enum UpdateEvent {
    Profile(PublicKey),
    Grasp(PublicKey),
    Repo(Update),
}

impl UpdateEvent {
    async fn next(
        notifications: &mut (impl futures::Stream<Item = ClientNotification> + Unpin),
        seen: &mut HashSet<EventId>,
    ) -> Option<Self> {
        loop {
            match notifications.next().await {
                Some(ClientNotification::Message { message, .. }) => {
                    let RelayMessage::Event { event, .. } = *message else {
                        continue;
                    };

                    let update = match event.kind {
                        Kind::Metadata => UpdateEvent::Profile(event.pubkey),
                        Kind::GitUserGraspList => UpdateEvent::Grasp(event.pubkey),
                        kind if filters::is_repo_kind(kind) => {
                            UpdateEvent::Repo(Update::from_event(&event))
                        }
                        _ => continue,
                    };

                    if seen.insert(event.id) {
                        return Some(update);
                    }
                }
                Some(_) => continue,
                None => return None,
            }
        }
    }
}

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

fn with_master_key(uri: &str, keys: &Keys) -> String {
    let separator = if uri.contains('?') { '&' } else { '?' };
    let nsec = keys.secret_key().to_bech32().expect("infallible");
    format!("{uri}{separator}master={nsec}")
}
