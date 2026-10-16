use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Error, anyhow, bail};
use bitcoin_hashes::sha1::Hash as Sha1Hash;
use gpui::{AppContext, AsyncApp};
use nostr::prelude::*;
use nostr_sdk::prelude::*;
use signed_core::Announcement;
use signed_git::Repo;

use crate::push::{GraspServer, PushOutcome, grasp_base_url};

/// A git ref with the commit it points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitRef {
    /// Full ref name, like `refs/heads/main` or `refs/tags/v1.0`.
    pub name: String,
    pub commit: Sha1Hash,
}

impl GitRef {
    pub fn new(name: impl Into<String>, commit: &str) -> Result<Self, Error> {
        let commit = Sha1Hash::from_str(commit).map_err(|_| anyhow!("invalid commit: {commit}"))?;

        Ok(Self {
            name: name.into(),
            commit,
        })
    }
}

/// How much to push to the grasp servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GitPushMode {
    /// Mirror all branches and tags.
    #[default]
    All,
    /// Push `refs/heads/main` only (repo creation).
    Main,
}

/// How to handle server-only refs when pushing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GitPushPolicy {
    /// Reconcile the local refs with the grasp servers,
    /// fetching missing refs before pushing.
    #[default]
    Reconcile,
    /// Declare the local refs only and delete server-only refs while pushing.
    ResetRemote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncState {
    pub refs: Vec<GitRef>,
    pub head: Option<String>,
    /// Local repository to push from; `None` for states parsed from events.
    pub path: Option<PathBuf>,
    pub mode: GitPushMode,
    pub policy: GitPushPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushStep {
    AnnounceRepository,
    /// Publish the state event to one grasp server.
    StageStateEvent(RelayUrl),
    /// Git push to one grasp server.
    PushObjects(RelayUrl),
    /// Fan the state event out to the relays.
    BroadcastStateEvent,
}

#[derive(Debug, Clone)]
pub enum PushStepStatus {
    Running(PushStep),
    Done(PushStep),
    Failed(PushStep, String),
}

impl PushStep {
    pub fn label(&self) -> String {
        match self {
            Self::AnnounceRepository => "Announce repository".to_owned(),
            Self::StageStateEvent(relay) => format!("Publish state event -> {relay}"),
            Self::BroadcastStateEvent => "Publish state event to relays".to_owned(),
            Self::PushObjects(relay) => format!(
                "Push objects -> {}",
                grasp_base_url(relay).unwrap_or_else(|| relay.to_string())
            ),
        }
    }
}

impl PushStepStatus {
    pub fn step(&self) -> &PushStep {
        match self {
            Self::Running(step) | Self::Done(step) | Self::Failed(step, _) => step,
        }
    }
}

impl SyncState {
    pub fn build(id: &str, refs: &[GitRef], head: Option<&str>) -> EventBuilder {
        let mut tags: Vec<Tag> = vec![Tag::identifier(id.to_owned())];

        for git_ref in refs {
            let commit = git_ref.commit.to_string();
            tags.push(Tag::parse([git_ref.name.as_str(), commit.as_str()]).expect("valid ref tag"));
        }

        if let Some(head) = head {
            tags.push(
                Tag::parse(["HEAD", &format!("ref: refs/heads/{head}")]).expect("valid HEAD tag"),
            );
        }

        EventBuilder::new(Kind::RepoState, "").tags(tags)
    }

    pub fn parse(event: &Event) -> Self {
        let mut refs = Vec::new();
        let mut head = None;

        for tag in event.tags.iter() {
            match Nip34Tag::parse(tag.as_slice()) {
                Ok(Nip34Tag::Head(branch)) => head = Some(branch),
                Ok(Nip34Tag::RefHead { branch, commit }) => {
                    refs.push(GitRef {
                        name: format!("refs/heads/{branch}"),
                        commit,
                    });
                }
                Ok(Nip34Tag::RefTag { name, commit }) => {
                    refs.push(GitRef {
                        name: format!("refs/tags/{name}"),
                        commit,
                    });
                }
                _ => {}
            }
        }

        Self {
            refs,
            head,
            path: None,
            mode: GitPushMode::default(),
            policy: GitPushPolicy::default(),
        }
    }

    /// Reads the refs and head of a local repository, ready to push.
    pub fn from_repo(path: &Path) -> Result<Self, Error> {
        let repo = Repo::open(path)?;
        let state = repo.ref_state()?;

        let refs = state
            .refs
            .iter()
            .map(|(name, commit)| GitRef::new(name, commit))
            .collect::<Result<Vec<_>, Error>>()?;

        Ok(Self {
            refs,
            head: state.head,
            path: Some(path.to_path_buf()),
            mode: GitPushMode::default(),
            policy: GitPushPolicy::default(),
        })
    }

    pub fn with_mode(mut self, mode: GitPushMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn with_policy(mut self, policy: GitPushPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Reconciles the local repository with the grasp servers, fetching missing refs.
    pub async fn reconcile(
        &mut self,
        announcement: &Announcement,
        cx: &mut AsyncApp,
    ) -> Result<(), Error> {
        if self.policy == GitPushPolicy::ResetRemote || self.refs.is_empty() {
            return Ok(());
        }

        let Some(path) = self.path.clone() else {
            return Ok(());
        };

        let local: Vec<(String, String)> = self
            .refs
            .iter()
            .map(|git_ref| (git_ref.name.clone(), git_ref.commit.to_string()))
            .collect();

        let urls: Vec<String> = announcement
            .clone
            .iter()
            .map(|url| url.to_string())
            .collect();

        let snapshot_path = path.clone();

        let missing = cx
            .background_spawn(async move {
                let repo = Repo::open(&path)?;

                // Refs the servers advertise that the local repository lacks
                let mut missing: Vec<(String, Vec<String>)> = Vec::new();
                // Diverged server tips that exist only on the servers
                let mut backups: Vec<(String, String, Vec<String>)> = Vec::new();

                for url in &urls {
                    let Ok(remote_refs) = repo.ls_remote(url) else {
                        continue;
                    };

                    for (name, oid) in remote_refs {
                        let Some((_, local_oid)) =
                            local.iter().find(|(local_name, _)| local_name == &name)
                        else {
                            match missing.iter_mut().find(|(known, _)| known == &name) {
                                Some((_, sources)) => sources.push(url.clone()),
                                None => missing.push((name, vec![url.clone()])),
                            }
                            continue;
                        };

                        if local_oid == &oid {
                            continue;
                        }

                        if repo.merge_base(local_oid, &oid).is_err() {
                            match backups.iter_mut().find(|(known, _, _)| known == &name) {
                                Some((_, _, sources)) => sources.push(url.clone()),
                                None => {
                                    backups.push((name.clone(), oid.clone(), vec![url.clone()]))
                                }
                            }
                        }
                    }
                }

                let mut failures = Vec::new();

                for (name, sources) in &missing {
                    let refspec = format!("+{name}:{name}");
                    if let Err(error) = repo.fetch_refs(sources, &refspec) {
                        log::warn!("failed to fetch {name} from grasp servers: {error}");
                        failures.push(name.clone());
                    }
                }

                for (name, oid, sources) in &backups {
                    let refspec = format!("+{name}:refs/nostr/backup/{oid}");
                    if let Err(error) = repo.fetch_refs(sources, &refspec) {
                        log::warn!("failed to back up {name} from grasp servers: {error}");
                    }
                }

                match failures.is_empty() {
                    true => Ok(missing
                        .into_iter()
                        .map(|(name, _)| name)
                        .collect::<Vec<_>>()),
                    false => bail!(
                        "could not fetch {} from any grasp server",
                        failures.join(", ")
                    ),
                }
            })
            .await?;

        if !missing.is_empty() {
            let (mode, policy) = (self.mode, self.policy);
            *self = Self::from_repo(&snapshot_path)?;
            self.mode = mode;
            self.policy = policy;
        }

        Ok(())
    }

    /// State event for these refs; the caller signs it.
    pub fn state_event(&self, repo_id: &str) -> EventBuilder {
        Self::build(repo_id, &self.refs, self.head.as_deref())
    }

    /// Push the state event and objects to the grasp servers.
    ///
    /// Calling `on_step` for each step.
    pub async fn push_to_grasps<P>(
        &self,
        client: &Client,
        event: Event,
        announcement: &Announcement,
        cx: &mut AsyncApp,
        on_step: &mut P,
    ) -> Result<PushOutcome, Error>
    where
        P: FnMut(PushStepStatus, &mut AsyncApp) -> Result<(), Error>,
    {
        let path = self
            .path
            .as_deref()
            .ok_or_else(|| anyhow!("push requires a local repository"))?;

        let owner = announcement
            .owner
            .to_bech32()
            .map_err(|error| anyhow!("could not encode the repository owner: {error}"))?;

        let mut outcome = PushOutcome::default();
        let policy = self.policy;
        let declared: Vec<String> = self
            .refs
            .iter()
            .map(|git_ref| git_ref.name.clone())
            .collect();

        for relay in &announcement.relays {
            let step = PushStep::StageStateEvent(relay.clone());

            // The relay must accept the state event before the git push is allowed.
            on_step(PushStepStatus::Running(step.clone()), cx)?;

            // Get base URL for the git push
            let base = grasp_base_url(relay).unwrap_or_default();

            // Ensure the relay is connected before sending the event.
            client.add_relay(relay).and_connect().await?;

            let staging = client.send_event(&event).to([relay.clone()]).await;
            // Whether the relay accepted the event
            let sent_failed = !staging
                .as_ref()
                .is_ok_and(|output| output.success.contains_key(relay));

            if sent_failed {
                let reason = match &staging {
                    Ok(output) => output
                        .failed
                        .get(relay)
                        .cloned()
                        .unwrap_or_else(|| "relay did not confirm the event".to_owned()),
                    Err(e) => format!("could not send the state event to {relay}: {e}"),
                };

                // Run the callback to report the failure
                on_step(PushStepStatus::Failed(step, reason.clone()), cx)?;

                // Record the failure in the outcome and continue to the next relay
                outcome.servers.push(GraspServer::failed(relay, reason));
                continue;
            }

            // Run the callback to report the success
            on_step(PushStepStatus::Done(step), cx)?;

            // Run the git push in the background, reporting progress through the callback
            let step = PushStep::PushObjects(relay.clone());
            on_step(PushStepStatus::Running(step.clone()), cx)?;

            let pushed = cx.background_spawn({
                let path = path.to_path_buf();
                let base = base.clone();
                let owner = owner.clone();
                let repo_id = announcement.id.clone();
                let mode = self.mode;
                let declared = declared.clone();

                async move {
                    let repo = Repo::open(&path)?;

                    match mode {
                        GitPushMode::Main => repo.push_main(&base, &owner, &repo_id),
                        GitPushMode::All => {
                            let mut refspecs = force_refspecs();
                            if policy == GitPushPolicy::ResetRemote {
                                let url = format!("{base}/{owner}/{repo_id}.git");
                                let server_refs = repo.ls_remote(&url)?;
                                refspecs.extend(deletion_refspecs(&declared, &server_refs));
                            }

                            let refspecs = refspecs.iter().map(String::as_str).collect::<Vec<_>>();
                            repo.push_refspecs(&base, &owner, &repo_id, &refspecs)
                        }
                    }
                }
            });

            match pushed.await {
                Ok(()) => {
                    // Run the callback to report the success
                    on_step(PushStepStatus::Done(step), cx)?;

                    outcome.state_event = Some(event.clone());
                    outcome.servers.push(GraspServer::ok(relay.clone()));
                }
                Err(e) => {
                    // Run the callback to report the failure
                    on_step(PushStepStatus::Failed(step, e.to_string()), cx)?;

                    outcome
                        .servers
                        .push(GraspServer::failed(relay, e.to_string()));
                }
            }
        }

        // Fan the state out to the relays once a git server holds the objects.
        if outcome.state_event.is_some() {
            let step = PushStep::BroadcastStateEvent;
            on_step(PushStepStatus::Running(step.clone()), cx)?;

            if let Err(e) = client.send_event(&event).broadcast().await {
                on_step(PushStepStatus::Failed(step, e.to_string()), cx)?;
                log::warn!("failed to broadcast repository state: {e}");
            } else {
                on_step(PushStepStatus::Done(step), cx)?;
            }
        }

        Ok(outcome)
    }

    /// Announce the repository and push the state and objects to the grasp servers.
    ///
    /// Both events are signed by the caller, so a failure at the call site can retract the announcement by id.
    pub async fn announce_and_push<P>(
        &self,
        client: &Client,
        announcement: Event,
        state_event: Event,
        cx: &mut AsyncApp,
        on_step: &mut P,
    ) -> Result<Event, Error>
    where
        P: FnMut(PushStepStatus, &mut AsyncApp) -> Result<(), Error>,
    {
        let step = PushStep::AnnounceRepository;
        on_step(PushStepStatus::Running(step.clone()), cx)?;

        let published = Announcement::from_event(&announcement)
            .ok_or_else(|| anyhow!("failed to parse announcement"))?;

        // Ensure the relays are connected before sending the announcement event.
        for url in &published.relays {
            client.add_relay(url).and_connect().await.ok();
        }

        let output = client.send_event(&announcement).broadcast().await?;

        // Handle the case where no relay accepted the announcement event.
        if output.success.is_empty() {
            let reasons = output
                .failed
                .values()
                .cloned()
                .collect::<Vec<String>>()
                .join(", ");

            // Run the callback to report the failure
            let step = PushStepStatus::Failed(step, format!("event not accepted: {reasons}"));
            on_step(step, cx)?;

            bail!("event not accepted: {reasons}");
        }

        on_step(PushStepStatus::Done(step), cx)?;

        let outcome = if self.refs.is_empty() {
            PushOutcome::default()
        } else {
            self.push_to_grasps(client, state_event, &published, cx, on_step)
                .await?
        };

        if outcome.accepted() == 0 {
            bail!(
                "Repository was announced, but the push to grasp servers failed: {}",
                outcome.failure_summary()
            );
        }

        Ok(announcement)
    }

    /// Retracts a live announcement; call when `announce_and_push` fails.
    pub async fn retract_announcement<S>(
        client: &Client,
        signer: &S,
        id: EventId,
    ) -> Result<(), Error>
    where
        S: AsyncGetPublicKey + AsyncSignEvent + ?Sized,
    {
        let builder = EventDeletionRequest::new().id(id);
        let deletion = builder.finalize_async(signer).await?;
        let output = client.send_event(&deletion).broadcast().await?;

        // Without relay acceptance the deletion request stays live.
        if output.success.is_empty() {
            bail!("deletion event not accepted by any relay");
        }

        Ok(())
    }
}

/// Refspecs to force push all heads and tags to the grasp servers.
fn force_refspecs() -> Vec<String> {
    vec![
        "+refs/heads/*:refs/heads/*".to_owned(),
        "+refs/tags/*:refs/tags/*".to_owned(),
    ]
}

/// Refspecs to delete server-only refs from the grasp servers.
fn deletion_refspecs(declared: &[String], server_refs: &[(String, String)]) -> Vec<String> {
    server_refs
        .iter()
        .filter(|(name, _)| !declared.contains(name))
        .map(|(name, _)| format!(":{name}"))
        .collect()
}
