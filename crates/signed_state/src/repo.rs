use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::{Error, bail};
use bitcoin_hashes::sha1::Hash as Sha1Hash;
use gpui::{App, AppContext, Context, SharedString, Subscription, Task};
use nostr::event::IntoEventBuilder;
use nostr_sdk::prelude::*;
use settings::{EventFetchingStrategy, SettingsStore};
use signed_core::{
    Announcement, Deletions, Filters, GitEvent, PullRequest, RepoAddr, RepoState, RepoStatus,
    filters,
};
use signed_git::{Nip34Binding, PatchParser, Repo};
use signed_nostr::UniversalSigner;

use crate::backend::{Backend, BackendEvent};
use crate::bootstrap::user_grasp_list_servers;
use crate::checkouts::CheckoutsStore;
use crate::push::{GraspPush, PushOutcome, grasp_base_url, grasp06_prs_url, pr_clone_urls};
use crate::repos::RepoListStore;

// NIP-34 suggests patches when each event is under 60kb.
const MAX_PATCH_EVENT_BYTES: usize = 60 * 1024;

pub struct RepoStore {
    addr: Option<RepoAddr>,
    // Seeded from the open-time hint, replaced by the database's latest on the
    // first pass. `None` while local-only.
    pub announcement: Option<Announcement>,
    // The scan path for a local repository, kept when it is later announced so
    // the panel keeps its worktree.
    pub path: Option<PathBuf>,
    pub nip34: Option<Nip34Binding>,
    // Views distinguish "no data yet" from a genuinely empty repository with it.
    pub loaded: bool,
    pub head: Option<String>,
    pub issues: Vec<Event>,
    pub patches: Vec<Event>,
    pub pull_requests: Vec<Event>,
    pub comments: Vec<Event>,
    status_by_root: HashMap<EventId, RepoStatus>,
    open_issue_count: usize,
    open_pr_count: usize,
    pub last_error: Option<String>,
    pub last_warning: Option<String>,
    // The repository is out of sync on the rejected servers until republished.
    pub last_push_warning: Option<String>,
    pub pushing: bool,
    pub cloning: bool,
    // Avoids re-subscribing and re-fetching on every refresh.
    repo_relays: HashSet<RelayUrl>,
    // Covers NIP-22 comments and statuses without an `a` tag.
    root_fetches: HashSet<EventId>,
    synced_maintainers: HashSet<PublicKey>,
    // In-flight tasks, cancelled when the store drops.
    tasks: Vec<Task<Result<(), Error>>>,
    _subscription: Option<Subscription>,
}

impl RepoStore {
    pub fn new(addr: RepoAddr, hint: Option<Announcement>, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        let subscription = Self::subscribe_backend(cx);

        let announced_relays = hint
            .as_ref()
            .map(|announcement| announcement.relays.clone())
            .unwrap_or_default();

        cx.defer(move |cx| {
            let result = weak.update(cx, |this, cx| {
                this.subscribe_remote(cx);
                this.connect_announced_relays(&announced_relays, cx);
                this.refresh(cx);
            });

            if let Err(error) = result {
                log::warn!("repo store dropped before bootstrap could run: {error}");
            }
        });

        Self {
            addr: Some(addr),
            announcement: hint,
            path: None,
            nip34: None,
            loaded: false,
            head: None,
            issues: Vec::new(),
            patches: Vec::new(),
            pull_requests: Vec::new(),
            comments: Vec::new(),
            status_by_root: HashMap::new(),
            open_issue_count: 0,
            open_pr_count: 0,
            last_error: None,
            last_warning: None,
            last_push_warning: None,
            pushing: false,
            cloning: false,
            repo_relays: HashSet::new(),
            root_fetches: HashSet::new(),
            synced_maintainers: HashSet::new(),
            tasks: Vec::new(),
            _subscription: Some(subscription),
        }
    }

    pub fn new_local(path: PathBuf, nip34: Option<Nip34Binding>) -> Self {
        Self {
            addr: None,
            announcement: None,
            path: Some(path),
            nip34,
            loaded: true,
            head: None,
            issues: Vec::new(),
            patches: Vec::new(),
            pull_requests: Vec::new(),
            comments: Vec::new(),
            status_by_root: HashMap::new(),
            open_issue_count: 0,
            open_pr_count: 0,
            last_error: None,
            last_warning: None,
            last_push_warning: None,
            pushing: false,
            cloning: false,
            repo_relays: HashSet::new(),
            root_fetches: HashSet::new(),
            synced_maintainers: HashSet::new(),
            tasks: Vec::new(),
            _subscription: None,
        }
    }

    pub fn from_worktree(
        addr: RepoAddr,
        announcement: Announcement,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut store = Self::new(addr, Some(announcement), cx);
        store.path = Some(path);
        store
    }

    pub fn announce(&mut self, announcement: Announcement, cx: &mut Context<Self>) {
        self.addr = Some(announcement.addr());
        self.announcement = Some(announcement.clone());
        self.loaded = false;

        if self._subscription.is_none() {
            self._subscription = Some(Self::subscribe_backend(cx));
        }

        self.subscribe_remote(cx);
        self.connect_announced_relays(&announcement.relays, cx);
        self.refresh(cx);
    }

    fn subscribe_backend(cx: &mut Context<Self>) -> Subscription {
        let backend = Backend::global(cx);

        cx.subscribe(&backend, |this, _backend, event, cx| {
            let Some(addr) = this.addr.as_ref() else {
                return;
            };

            let relevant = match event {
                BackendEvent::RepoUpdates(updates) => updates.iter().any(|update| {
                    let deletion =
                        update.kind == Kind::EventDeletion || update.kind == Kind::RequestToVanish;

                    let coordinate = update.coordinate.as_ref() == Some(addr.coordinate());
                    let author = update.author == addr.public_key();

                    let authored = (update.kind == Kind::GitRepoAnnouncement
                        || update.kind == Kind::RepoState)
                        && author;

                    let comment = update.kind == Kind::Comment;
                    let status = RepoStatus::from_kind(update.kind).is_some();

                    deletion || coordinate || authored || comment || status
                }),
                _ => false,
            };

            if relevant {
                this.refresh(cx);
            }
        })
    }

    pub fn addr(&self) -> Option<&RepoAddr> {
        self.addr.as_ref()
    }

    pub fn name(&self) -> SharedString {
        self.announcement
            .as_ref()
            .map_or(SharedString::default(), |a| {
                SharedString::from(a.name.as_deref().unwrap_or("Unknown"))
            })
    }

    fn repo_filters(addr: &RepoAddr) -> Vec<Filter> {
        let mut filters = vec![
            Filter::new()
                .kinds([Kind::GitRepoAnnouncement, Kind::RepoState])
                .author(addr.public_key())
                .identifier(addr.identifier()),
            addr.activity_filter(),
        ];
        // Deletion requests must be known before any event is shown.
        filters.extend(addr.deletion_filters());
        filters
    }

    fn connect_announced_relays(&mut self, relays: &[RelayUrl], cx: &mut Context<Self>) {
        let Some(addr) = self.addr.clone() else {
            return;
        };

        let new: Vec<RelayUrl> = relays
            .iter()
            .filter(|url| !self.repo_relays.contains(*url))
            .cloned()
            .collect();

        if new.is_empty() {
            return;
        }
        self.repo_relays.extend(new.iter().cloned());

        let backend = Backend::global(cx);
        let filters = Self::repo_filters(&addr);

        backend.update(cx, |backend, cx| {
            backend.connect_repo_relays(new, filters, cx);
        });
    }

    // NIP-34 events tag the announcement author, which may not be a
    // maintainer for subordinate forks.
    fn maintainer_filters(addr: &RepoAddr, maintainers: &[PublicKey]) -> Vec<Filter> {
        let mut pubkeys = maintainers.to_vec();
        if !pubkeys.contains(&addr.public_key()) {
            pubkeys.push(addr.public_key());
        }

        vec![
            Filter::new()
                .kinds([Kind::GitRepoAnnouncement, Kind::RepoState])
                .authors(pubkeys.clone())
                .identifier(addr.identifier()),
            Filter::new()
                .kinds(filters::ACTIVITY_KINDS)
                .coordinate(addr.coordinate())
                .pubkeys(pubkeys.clone()),
            Filter::new()
                .kinds(filters::ACTIVITY_KINDS)
                .coordinate(addr.coordinate())
                .authors(pubkeys.clone()),
            Filter::new()
                .kinds([Kind::EventDeletion, Kind::RequestToVanish])
                .authors(pubkeys),
        ]
    }

    fn sync_maintainer_relays(&mut self, maintainers: &[PublicKey], cx: &mut Context<Self>) {
        let strategy = SettingsStore::try_global(cx)
            .map(|store| store.read(cx).settings().event_fetching)
            .unwrap_or_default();
        if strategy != EventFetchingStrategy::Uncensored {
            return;
        }

        let Some(addr) = self.addr.clone() else {
            return;
        };

        if !maintainers
            .iter()
            .any(|public_key| !self.synced_maintainers.contains(public_key))
        {
            return;
        }
        self.synced_maintainers.extend(maintainers.iter().copied());

        let filters = Self::maintainer_filters(&addr, maintainers);
        let backend = Backend::global(cx);

        backend.update(cx, |backend, cx| {
            backend.sync_auto(filters, cx);
        });
    }

    fn subscribe_remote(&mut self, cx: &mut Context<Self>) {
        let Some(addr) = self.addr.clone() else {
            return;
        };

        let backend = Backend::global(cx);

        backend.update(cx, |backend, cx| {
            backend.subscribe_bootstrap(Self::repo_filters(&addr), cx);
        });
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.addr.is_none() {
            return;
        }
        self.run_refresh(cx);
    }

    fn run_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(addr) = self.addr.clone() else {
            return;
        };

        let backend = Backend::global(cx);
        let client = backend.read(cx).client();

        let work = cx.background_spawn(async move {
            let (announcements, states, activity, deletion_events) = async {
                let db = client.database();
                let announcements = db.query(addr.announcement_filter()).await?;
                let states = db.query(addr.state_filter()).await?;
                let activity = db.query(addr.activity_filter()).await?;
                let deletion_events = db.query(Filters::deletions()).await?;

                Ok::<_, Error>((announcements, states, activity, deletion_events))
            }
            .await?;

            let deletions = Deletions::from_events(deletion_events);

            let all_announcements = announcements
                .into_iter()
                .filter(|e| !deletions.is_deleted(e));

            let announcement = utils::latest(all_announcements)
                .as_ref()
                .and_then(Announcement::from_event);

            let all_states = states.into_iter().filter(|e| !deletions.is_deleted(e));
            let state = utils::latest(all_states).map(|state| RepoState::parse(&state));

            let (mut issues, mut patches, mut pull_requests, mut statuses, mut comments) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());

            for event in activity {
                if deletions.is_deleted(&event) {
                    continue;
                }
                match event.kind {
                    Kind::GitIssue => issues.push(event),
                    Kind::GitPatch => patches.push(event),
                    Kind::GitPullRequest | Kind::GitPullRequestUpdate => pull_requests.push(event),
                    Kind::Comment => comments.push(event),
                    kind if RepoStatus::from_kind(kind).is_some() => statuses.push(event),
                    _ => {}
                }
            }

            let mut seen_comments: HashSet<EventId> = comments.iter().map(|e| e.id).collect();
            let db = client.database();

            let roots = issues
                .iter()
                .chain(&patches)
                .chain(&pull_requests)
                .map(|e| e.id);

            for filter in Filters::comments_for(roots) {
                for event in db.query(filter).await? {
                    if seen_comments.insert(event.id) {
                        comments.push(event);
                    }
                }
            }

            let mut seen_statuses: HashSet<EventId> = statuses.iter().map(|e| e.id).collect();
            let db = client.database();

            let roots = issues
                .iter()
                .chain(&patches)
                .chain(&pull_requests)
                .map(|e| e.id);

            for root in roots {
                for event in db.query(Filters::statuses_for([root])).await? {
                    if seen_statuses.insert(event.id) {
                        statuses.push(event);
                    }
                }
            }

            utils::sort_newest_first(&mut issues);
            utils::sort_newest_first(&mut patches);
            utils::sort_newest_first(&mut pull_requests);
            comments.sort_by_key(|comment| comment.created_at);

            let maintainers = announcement
                .as_ref()
                .map(Announcement::effective_maintainers)
                .unwrap_or_default();

            let status_by_root =
                resolve_statuses(&issues, &patches, &pull_requests, &statuses, &maintainers);

            let open_issue_count = issues
                .iter()
                .filter(|issue| status_of(&status_by_root, issue) == RepoStatus::Open)
                .count();

            let open_pr_count = pull_requests
                .iter()
                .filter(|pr| {
                    pr.kind == Kind::GitPullRequest
                        && status_of(&status_by_root, pr) == RepoStatus::Open
                })
                .count();

            Ok::<_, Error>((
                announcement,
                state,
                issues,
                patches,
                pull_requests,
                status_by_root,
                open_issue_count,
                open_pr_count,
                comments,
            ))
        });

        let task = cx.spawn(async move |this, cx| {
            let (
                announcement,
                state,
                issues,
                patches,
                pull_requests,
                status_by_root,
                open_issue_count,
                open_pr_count,
                comments,
            ) = match work.await {
                Ok(data) => data,
                Err(e) => {
                    return this.update(cx, |this, cx| {
                        this.last_error = Some(e.to_string());
                        cx.notify();
                    });
                }
            };

            this.update(cx, |this, cx| {
                let keep_hint = announcement.is_none() && !this.loaded;
                let first_pass = !this.loaded;

                let head_changed = state
                    .as_ref()
                    .is_some_and(|state| this.head.as_deref() != state.head.as_deref());

                let changed = first_pass
                    || (!keep_hint && this.announcement != announcement)
                    || head_changed
                    || this.issues != issues
                    || this.patches != patches
                    || this.pull_requests != pull_requests
                    || this.comments != comments
                    || this.status_by_root != status_by_root;

                if !keep_hint {
                    this.announcement = announcement;
                }

                let relays = this
                    .announcement
                    .as_ref()
                    .map(|a| a.relays.clone())
                    .unwrap_or_default();

                this.connect_announced_relays(&relays, cx);

                let maintainers = this
                    .announcement
                    .as_ref()
                    .map(Announcement::effective_maintainers)
                    .unwrap_or_default();

                this.sync_maintainer_relays(&maintainers, cx);

                if let Some(state) = state {
                    this.head = state.head;
                }

                this.issues = issues;
                this.patches = patches;
                this.pull_requests = pull_requests;
                this.comments = comments;
                this.status_by_root = status_by_root;
                this.open_issue_count = open_issue_count;
                this.open_pr_count = open_pr_count;
                this.loaded = true;

                let roots = this
                    .issues
                    .iter()
                    .chain(&this.patches)
                    .chain(&this.pull_requests)
                    .map(|e| e.id)
                    .collect::<HashSet<EventId>>();

                let new_roots: Vec<EventId> = roots
                    .iter()
                    .filter(|id| !this.root_fetches.contains(id))
                    .copied()
                    .collect();

                if !new_roots.is_empty() {
                    this.root_fetches.extend(new_roots.iter().copied());

                    let mut root_filters = Filters::comments_for(new_roots.clone());
                    root_filters.push(Filters::statuses_for(new_roots.iter().copied()));

                    let announced: Vec<RelayUrl> = this.repo_relays.iter().cloned().collect();
                    let backend = Backend::global(cx);

                    backend.update(cx, |backend, cx| {
                        backend.subscribe_bootstrap(root_filters.clone(), cx);
                        backend.connect_repo_relays(announced, root_filters, cx);
                    });
                }

                if changed {
                    cx.notify();
                }
            })?;

            Ok(())
        });

        self.tasks.push(task);
    }

    pub fn status_of(&self, root: &Event) -> RepoStatus {
        status_of(&self.status_by_root, root)
    }

    pub fn issue_count(&self) -> usize {
        self.open_issue_count
    }

    pub fn pull_request_count(&self) -> usize {
        self.open_pr_count
    }

    pub fn is_author(&self, user: &PublicKey) -> bool {
        self.addr
            .as_ref()
            .is_some_and(|addr| addr.public_key() == *user)
    }

    pub fn open_issue(&mut self, subject: Option<String>, content: String, cx: &mut Context<Self>) {
        let Some(addr) = self.addr.clone() else {
            self.not_announced(cx);
            return;
        };

        let builder = GitIssue {
            repository: addr.into(),
            content,
            subject,
            labels: Vec::new(),
        }
        .into_event_builder();

        self.publish(builder, cx);
    }

    pub fn comments_of(&self, root: &EventId) -> impl Iterator<Item = &Event> {
        self.comments
            .iter()
            .filter(move |e| e.references_root(root))
    }

    pub fn comment(&mut self, root: &Event, content: String, cx: &mut Context<Self>) {
        self.reply(root, None, content, cx);
    }

    fn reply(
        &mut self,
        root: &Event,
        parent: Option<&Event>,
        content: String,
        cx: &mut Context<Self>,
    ) {
        let Some(addr) = self.addr.clone() else {
            self.not_announced(cx);
            return;
        };

        let relay_hint = self
            .announcement
            .as_ref()
            .and_then(|a| a.relays.first())
            .cloned();

        self.publish(
            comment_builder(root, parent, relay_hint.as_ref(), &addr, content),
            cx,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open_pull_request(
        &mut self,
        subject: Option<String>,
        description: String,
        branch_name: Option<String>,
        patch: String,
        draft: bool,
        merge_base: Option<String>,
        push_from: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.last_error = None;
        self.last_warning = None;

        let Some(addr) = self.addr.clone() else {
            self.not_announced(cx);
            return;
        };

        let series = PatchSeries::parse(&patch);

        if let Some(oversized) = series.oversized_length() {
            self.last_error = Some(format!(
                "patch too large ({} bytes; NIP-34 suggests keeping each patch under {} bytes)",
                oversized, MAX_PATCH_EVENT_BYTES
            ));
            cx.notify();
            return;
        }

        // The tip of the series is its last commit; `git format-patch` orders
        // patches oldest first.
        let Some(current_commit) = series.tip_commit() else {
            self.last_error = Some(
                "Patch must be `git format-patch` output with a `From <commit-id>` header".into(),
            );
            cx.notify();
            return;
        };

        let backend = Backend::global(cx);
        let (signer, client, user) = {
            let backend = backend.read(cx);
            (backend.signer(), backend.client(), backend.current_user())
        };

        let Some(user) = user else {
            self.last_error = Some("Sign in to open a pull request".into());
            cx.notify();
            return;
        };
        let author_npub = user.to_bech32().unwrap();

        let owner = addr.public_key();
        let euc = self.announcement.as_ref().and_then(|a| a.euc.clone());
        let repo_id = addr.identifier().to_owned();
        let base_npub = owner.to_bech32().unwrap();
        let push_relays = self
            .announcement
            .as_ref()
            .map(|a| a.relays.clone())
            .unwrap_or_default();

        // GRASP-06 hosting falls back to the settings defaults.
        // That happens when the author has no published grasp list.
        let defaults: Vec<RelayUrl> = {
            let settings = settings::SettingsStore::global(cx).read(cx).settings();
            let urls: Vec<String> = if settings.grasp_servers.default_servers.is_empty() {
                settings::DEFAULT_GRASP_SERVERS
                    .iter()
                    .map(|url| (*url).to_owned())
                    .collect()
            } else {
                settings.grasp_servers.default_servers.clone()
            };
            urls.iter()
                .filter_map(|url| RelayUrl::parse(url).ok())
                .collect()
        };

        let task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            let root_patch = match publish_patch_series(
                &client,
                &signer,
                &addr,
                owner,
                euc.as_deref(),
                &series,
                "root",
                None,
            )
            .await
            {
                Ok(event) => event,
                Err(e) => {
                    return this.update(cx, |this, cx| {
                        this.last_error = Some(e.to_string());
                        cx.notify();
                    });
                }
            };

            // GRASP-06 pushes the tip to the author's own grasp servers at
            // `/prs/<author-npub>/<repo-id>.git`; contributing to another
            // project never depends on that project's servers. Resolve the
            // servers from the author's latest kind-10317 grasp list.
            let author_servers = {
                let query_client = client.clone();
                let published = cx
                    .background_spawn(
                        async move { user_grasp_list_servers(&query_client, user).await },
                    )
                    .await;

                match published {
                    Ok(published) if !published.is_empty() => published,
                    _ => defaults,
                }
            };

            let author_targets: Vec<(String, String)> = {
                let mut targets = Vec::new();
                for server in &author_servers {
                    let Some(base) = grasp_base_url(server) else {
                        continue;
                    };
                    let url = grasp06_prs_url(&base, &author_npub, &repo_id);
                    if !targets.iter().any(|(existing, _)| existing == &url) {
                        targets.push((url, server.to_string()));
                    }
                }
                targets
            };
            let base_targets: Vec<(String, String)> = {
                let mut targets = Vec::new();
                for relay in &push_relays {
                    let Some(base) = grasp_base_url(relay) else {
                        continue;
                    };
                    let url = format!("{base}/{base_npub}/{repo_id}.git");
                    if !targets.iter().any(|(existing, _)| existing == &url) {
                        targets.push((url, relay.to_string()));
                    }
                }
                targets
            };

            let builder = this.update(cx, |this, _cx| {
                let prs_urls: Vec<Url> = author_targets
                    .iter()
                    .filter_map(|(url, _)| Url::parse(url).ok())
                    .collect();

                let base_clone = this
                    .announcement
                    .as_ref()
                    .map(|a| a.clone.clone())
                    .unwrap_or_default();

                let clone = pr_clone_urls(prs_urls, base_clone);

                let builder = GitPullRequest {
                    repository: addr.clone().into(),
                    content: description,
                    subject,
                    labels: Vec::new(),
                    branch_name,
                    clone,
                    current_commit,
                    root_patch_event: Some(root_patch.id),
                    merge_base: merge_base
                        .and_then(|hex| hex.parse::<bitcoin_hashes::Sha1>().ok()),
                }
                .into_event_builder();

                // The `r` EUC tag lets clients subscribe to all PRs of the repository.
                match this.announcement.as_ref().and_then(|a| a.euc.clone()) {
                    Some(euc) => builder.tag(Tag::parse(["r", &euc]).expect("valid r tag")),
                    None => builder,
                }
            })?;

            let event = cx
                .background_spawn({
                    let signer = signer.clone();
                    async move { builder.finalize_async(&signer).await }
                })
                .await?;

            if let Some(path) = push_from.as_ref() {
                let tip = current_commit.to_string();
                let reference = format!("refs/nostr/{}", event.id.to_hex());

                let (pushed, failures) = cx
                    .background_spawn({
                        let path = path.clone();
                        let tip = tip.clone();
                        let reference = reference.clone();
                        let targets: Vec<(String, String)> = author_targets
                            .into_iter()
                            .chain(base_targets)
                            .collect();

                        async move {
                            let mut failures = Vec::new();
                            let mut pushed = 0;

                            for (url, label) in &targets {
                                match Repo::open(&path)
                                    .and_then(|repo| repo.push_ref(url, &tip, &reference))
                                {
                                    Ok(()) => pushed += 1,
                                    Err(e) => failures.push(format!("{label}: {e}")),
                                }
                            }

                            (pushed, failures)
                        }
                    })
                    .await;

                if pushed == 0 {
                    this.update(cx, |this, cx| {
                        this.last_warning = Some(format!(
                            "Pull request published, but the commit could not be pushed to any grasp server ({}); the patch is still the source of truth",
                            failures.join("; ")
                        ));
                        cx.notify();
                    })?;
                }
            }

            let publish_result = {
                let pusher = GraspPush::new(client.clone(), signer.clone());
                pusher.send_accepted(event).await
            };

            let pr_event = match publish_result {
                Ok(event) => event,
                Err(e) => {
                    return this.update(cx, |this, cx| {
                        this.last_error = Some(e.to_string());
                        cx.notify();
                    });
                }
            };

            if draft {
                this.update(cx, |this, cx| {
                    this.set_status(&pr_event, RepoStatus::Draft, cx);
                })?;
            }

            Ok(())
        });
        self.tasks.push(task);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn open_pull_request_from_refs(
        &mut self,
        repo_path: PathBuf,
        merge_base: String,
        compare_ref: String,
        subject: Option<String>,
        description: String,
        branch_name: Option<String>,
        draft: bool,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), Error>> {
        cx.spawn(async move |this, cx| {
            // Regenerate at submit time so the published patch covers the
            // current tip of the compare branch.
            let patch = cx
                .background_spawn({
                    let repo_path = repo_path.clone();
                    let merge_base = merge_base.clone();
                    let compare_ref = compare_ref.clone();
                    async move {
                        Repo::open(&repo_path)?.format_patch_between(&merge_base, &compare_ref)
                    }
                })
                .await;

            let patch = match patch {
                Ok(patch) if !patch.is_empty() => patch,
                Ok(_) => bail!("No commits between the branches to propose"),
                Err(error) => bail!("Failed to generate the patch: {error}"),
            };

            this.update(cx, |this, cx| {
                this.open_pull_request(
                    subject,
                    description,
                    branch_name,
                    patch,
                    draft,
                    Some(merge_base),
                    Some(repo_path),
                    cx,
                );
            })
        })
    }

    pub fn update_pull_request(&mut self, root: &Event, patch: String, cx: &mut Context<Self>) {
        self.last_error = None;
        self.last_warning = None;

        let backend = Backend::global(cx);
        let (user, client, signer) = {
            let backend = backend.read(cx);
            (backend.current_user(), backend.client(), backend.signer())
        };

        let Some(user) = user else {
            self.last_error = Some("Sign in to update the pull request".into());
            cx.notify();
            return;
        };

        if user != root.pubkey {
            self.last_error = Some("Only the pull request author can update it".into());
            cx.notify();
            return;
        }

        let series = PatchSeries::parse(&patch);
        if let Some(oversized) = series.oversized_length() {
            self.last_error = Some(format!(
                "patch too large ({} bytes; NIP-34 suggests keeping each patch under {} bytes)",
                oversized, MAX_PATCH_EVENT_BYTES
            ));
            cx.notify();
            return;
        }

        // The tip of the updated PR is the last commit of the series.
        let Some(current_commit) = series.tip_commit() else {
            self.last_error = Some(
                "Patch must be `git format-patch` output with a `From <commit-id>` header".into(),
            );
            cx.notify();
            return;
        };

        // The first revision patch replies to the original root patch, NIP-34:
        // use the PR's `e` tag, or the oldest patch of the linked set.
        let root_patch_id = root.tags.event_ids().next().or_else(|| {
            PullRequest::new(root)
                .patches(self.patches.iter())
                .first()
                .map(|p| p.id)
        });

        let Some(addr) = self.addr.clone() else {
            self.not_announced(cx);
            return;
        };

        let owner = addr.public_key();
        let euc = self.announcement.as_ref().and_then(|a| a.euc.clone());
        let root = root.clone();

        let clone: Vec<Url> = self
            .announcement
            .as_ref()
            .map(|a| a.clone.clone())
            .unwrap_or_default();

        let task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            if let Err(e) = publish_patch_series(
                &client,
                &signer,
                &addr,
                owner,
                euc.as_deref(),
                &series,
                "root-revision",
                root_patch_id,
            )
            .await
            {
                return this.update(cx, |this, cx| {
                    this.last_error = Some(e.to_string());
                    cx.notify();
                });
            }

            let builder = {
                let builder = GitPullRequestUpdate {
                    repository: addr.clone().into(),
                    pull_request_event: root.id,
                    pull_request_author: root.pubkey,
                    current_commit,
                    clone: clone.clone(),
                    merge_base: None,
                }
                .into_event_builder();

                // The `r` EUC tag lets clients subscribe to all PR updates; the
                // SDK builder omits it.
                match euc.as_deref() {
                    Some(euc) => builder.tag(Tag::parse(["r", euc]).expect("valid r tag")),
                    None => builder,
                }
            };

            let publish_result = {
                let pusher = GraspPush::new(client.clone(), signer.clone());
                pusher.publish_one(builder).await
            };

            if let Err(e) = publish_result {
                return this.update(cx, |this, cx| {
                    this.last_error = Some(e.to_string());
                    cx.notify();
                });
            }

            Ok(())
        });
        self.tasks.push(task);
    }

    fn set_status(&mut self, root: &Event, status: RepoStatus, cx: &mut Context<Self>) {
        self.last_error = None;

        let Some(addr) = self.addr.clone() else {
            self.not_announced(cx);
            return;
        };

        let maintainers = self
            .announcement
            .as_ref()
            .map(Announcement::effective_maintainers)
            .unwrap_or_default();

        let backend = Backend::global(cx);
        let Some(user) = backend.read(cx).current_user() else {
            self.last_error = Some("Sign in to change the status".into());
            cx.notify();
            return;
        };

        if user != root.pubkey && !maintainers.contains(&user) {
            self.last_error = Some("Only the author or a maintainer can change the status".into());
            cx.notify();
            return;
        }

        let Ok(root_ref) = Tag::parse(["e", &root.id.to_hex(), "", "root"]) else {
            return;
        };

        let builder = EventBuilder::new(status.kind(), "").tags([
            root_ref,
            Tag::public_key(addr.public_key()),
            Tag::public_key(root.pubkey),
            Tag::coordinate(addr.clone().into(), None),
        ]);

        self.publish(builder, cx);
    }

    fn action_announcement(&self, cx: &App) -> Option<Announcement> {
        let addr = self.addr.as_ref()?;
        self.announcement.clone().or_else(|| {
            RepoListStore::global(cx)
                .read(cx)
                .announcements
                .iter()
                .find(|announcement| announcement.addr() == *addr)
                .cloned()
        })
    }

    pub fn push_repository(&mut self, cx: &mut Context<Self>) -> Task<Result<(), Error>> {
        if self.pushing {
            return Task::ready(Err(anyhow::anyhow!(
                "A push to this repository is already in progress"
            )));
        }
        let Some(announcement) = self.action_announcement(cx) else {
            return self.action_error("Repository announcement is not loaded yet", cx);
        };

        let backend = Backend::global(cx);
        let push = backend.update(cx, |backend, cx| backend.push_repository(announcement, cx));

        self.run_push(push, None, cx)
    }

    pub fn push_checkout(
        &mut self,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), Error>> {
        if self.pushing {
            return Task::ready(Err(anyhow::anyhow!(
                "A push to this repository is already in progress"
            )));
        }

        let Some(addr) = self.addr.clone() else {
            return self.action_error("This repository is not published to Nostr yet", cx);
        };

        let Some(announcement) = self.action_announcement(cx) else {
            return self.action_error("Repository announcement is not loaded yet", cx);
        };

        // The state event's `HEAD` stays the announced default branch.
        // The checkout may be on a side branch.
        let head = self.head.clone();

        let backend = Backend::global(cx);
        let push = backend.update(cx, |backend, cx| {
            backend.push_checkout(announcement, path.clone(), head, cx)
        });

        self.run_push(push, Some((addr, path)), cx)
    }

    fn run_push(
        &mut self,
        push: Task<Result<PushOutcome, Error>>,
        pushed_checkout: Option<(RepoAddr, PathBuf)>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), Error>> {
        self.pushing = true;
        self.last_error = None;
        self.last_push_warning = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = push.await;

            this.update(cx, |this, cx| {
                this.pushing = false;

                match &result {
                    Ok(outcome) => {
                        this.last_error = None;
                        this.last_push_warning = outcome.partial_warning();
                        if let Some((addr, path)) = &pushed_checkout {
                            CheckoutsStore::global(cx).update(cx, |store, cx| {
                                store.checkout_pushed(addr, path, cx);
                            });
                        }
                    }
                    Err(e) => {
                        this.last_error = Some(format!("Push failed: {e}"));
                        this.last_push_warning = None;
                    }
                }

                cx.notify();
            })?;

            result.map(|_| ())
        })
    }

    pub fn delete_repository(&mut self, cx: &mut Context<Self>) -> Task<Result<(), Error>> {
        let Some(addr) = self.addr.clone() else {
            return self.action_error("This repository is not published to Nostr yet", cx);
        };
        self.last_error = None;

        let backend = Backend::global(cx);
        let delete = backend.update(cx, |backend, cx| backend.delete_repository(addr, cx));

        cx.spawn(async move |this, cx| {
            let result = delete.await;
            this.update(cx, |this, cx| {
                if let Err(e) = &result {
                    this.last_error = Some(format!("Delete failed: {e}"));
                }
                cx.notify();
            })?;
            result
        })
    }

    // A user-chosen folder outside the cache; remembered as a checkout.
    pub fn clone_to_folder(
        &mut self,
        destination: PathBuf,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), Error>> {
        if self.cloning {
            return Task::ready(Err(anyhow::anyhow!(
                "A clone of this repository is already in progress"
            )));
        }

        let Some(addr) = self.addr.clone() else {
            return self.action_error("This repository is not published to Nostr yet", cx);
        };

        let Some(announcement) = self.action_announcement(cx) else {
            return self.action_error("Repository announcement is not loaded yet", cx);
        };

        let clone_urls = announcement.clone.clone();

        self.cloning = true;
        self.last_error = None;
        cx.notify();

        let clone = {
            let destination = destination.clone();
            // gix handles are not `Send` and must not cross the spawn boundary.
            cx.background_spawn(async move { Repo::clone(&clone_urls, &destination).map(|_| ()) })
        };

        cx.spawn(async move |this, cx| {
            let result = clone.await;

            this.update(cx, |this, cx| {
                this.cloning = false;

                match &result {
                    Ok(()) => {
                        let checkouts = CheckoutsStore::global(cx);
                        checkouts.update(cx, |store, cx| {
                            store.record(destination.clone(), addr.clone(), cx);
                        });
                    }
                    Err(e) => {
                        this.last_error = Some(format!("Failed to clone: {e}"));
                    }
                }

                cx.notify();
            })?;

            result
        })
    }

    fn not_announced(&mut self, cx: &mut Context<Self>) {
        self.last_error = Some("This repository is not published to Nostr yet".into());
        cx.notify();
    }

    fn action_error(
        &mut self,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), Error>> {
        let message = message.into();
        self.last_error = Some(message.clone());
        cx.notify();

        Task::ready(Err(anyhow::anyhow!("{message}")))
    }

    // Every one-shot repository event (issue, comment, status) goes through
    // this; multi-step flows call the SDK directly instead.
    fn publish(&mut self, builder: EventBuilder, cx: &mut Context<Self>) {
        self.last_error = None;

        let backend = Backend::global(cx);
        let (client, signer) = {
            let backend = backend.read(cx);
            (backend.client(), backend.signer())
        };

        let task: Task<Result<(), Error>> = cx.spawn(async move |this, cx| {
            let pusher = GraspPush::new(client, signer);
            let publish_result = pusher.publish_one(builder).await;

            if let Err(e) = publish_result {
                this.update(cx, |this, cx| {
                    this.last_error = Some(e.to_string());
                    cx.notify();
                })?;
            }

            Ok(())
        });
        self.tasks.push(task);
    }
}

fn status_of(status_by_root: &HashMap<EventId, RepoStatus>, root: &Event) -> RepoStatus {
    status_by_root
        .get(&root.id)
        .copied()
        .unwrap_or(RepoStatus::Open)
}

fn resolve_statuses(
    issues: &[Event],
    patches: &[Event],
    pull_requests: &[Event],
    statuses: &[Event],
    maintainers: &[PublicKey],
) -> HashMap<EventId, RepoStatus> {
    let mut by_root: HashMap<EventId, Vec<&Event>> = HashMap::new();
    for event in statuses {
        for tag in event.tags.iter() {
            if matches!(tag.kind(), "e" | "E")
                && let Some(id) = tag.content().and_then(|hex| EventId::from_hex(hex).ok())
            {
                by_root.entry(id).or_default().push(event);
            }
        }
    }

    issues
        .iter()
        .chain(patches)
        .chain(pull_requests)
        .map(|root| {
            let events = by_root.get(&root.id).map(Vec::as_slice).unwrap_or(&[]);
            let status =
                signed_core::RepoStatus::resolve(events.iter().copied(), &root.pubkey, maintainers);
            (root.id, status)
        })
        .collect()
}

// The `From <commit>` header on the first line.
fn patch_current_commit(patch: &str) -> Option<&str> {
    let line = patch.lines().next()?;
    let hex = line.strip_prefix("From ")?;
    hex.split_whitespace().next().filter(|hex| hex.len() == 40)
}

struct PatchSeries {
    parts: Vec<String>,
}

impl PatchSeries {
    fn parse(patch: &str) -> Self {
        Self {
            parts: PatchParser::split_patch_series(patch)
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
    }

    // The byte length of the first part over the NIP-34 size suggestion.
    fn oversized_length(&self) -> Option<usize> {
        self.parts
            .iter()
            .map(String::len)
            .find(|length| *length > MAX_PATCH_EVENT_BYTES)
    }

    // The tip of the series is its last commit; `git format-patch` orders
    // patches oldest first.
    fn tip_commit(&self) -> Option<Sha1Hash> {
        self.parts
            .last()
            .and_then(|part| patch_current_commit(part))
            .and_then(|hex| hex.parse::<Sha1Hash>().ok())
    }

    fn commit_of(&self, index: usize) -> Option<&str> {
        self.parts
            .get(index)
            .and_then(|part| patch_current_commit(part))
            .filter(|hex| hex.len() == 40)
    }
}

// Returns the root event, the one a PR references.
#[allow(clippy::too_many_arguments)]
async fn publish_patch_series(
    client: &Client,
    signer: &UniversalSigner,
    addr: &RepoAddr,
    owner: PublicKey,
    euc: Option<&str>,
    series: &PatchSeries,
    first_marker: &str,
    reply_to: Option<EventId>,
) -> Result<Event, Error> {
    let pusher = GraspPush::new(client.clone(), signer.clone());
    let mut root: Option<Event> = None;
    let mut previous = reply_to;

    for (ix, part) in series.parts.iter().enumerate() {
        let Some(commit) = series.commit_of(ix) else {
            return Err(anyhow::anyhow!(
                "patch {} of the series has no `From <commit-id>` header",
                ix + 1
            ));
        };

        let mut tags = vec![
            Tag::coordinate(addr.clone().into(), None),
            Tag::public_key(owner),
        ];

        if ix == 0 {
            if let Ok(tag) = Tag::parse(["t", first_marker]) {
                tags.push(tag);
            }
            if let Some(root_id) = reply_to
                && let Ok(tag) = Tag::parse(["e", &root_id.to_hex(), "", "reply"])
            {
                tags.push(tag);
            }
        } else if let Some(previous) = previous
            && let Ok(tag) = Tag::parse(["e", &previous.to_hex(), "", "reply"])
        {
            tags.push(tag);
        }

        if let Some(euc) = euc
            && let Ok(tag) = Tag::parse(["r", euc])
        {
            tags.push(tag);
        }

        if let Ok(tag) = Tag::parse(["commit", commit]) {
            tags.push(tag);
        }

        if let Ok(tag) = Tag::parse(["r", commit]) {
            tags.push(tag);
        }

        let builder = EventBuilder::new(Kind::GitPatch, part.clone()).tags(tags);
        let event = pusher.publish_one(builder).await?;

        if root.is_none() {
            root = Some(event.clone());
        }
        previous = Some(event.id);
    }

    root.ok_or_else(|| anyhow::anyhow!("patch series is empty"))
}

fn comment_builder(
    root: &Event,
    parent: Option<&Event>,
    relay_hint: Option<&RelayUrl>,
    addr: &RepoAddr,
    content: String,
) -> EventBuilder {
    let target = |event: &Event| {
        CommentTarget::event(
            event.id,
            event.kind,
            Some(event.pubkey),
            relay_hint.cloned().map(Cow::Owned),
        )
    };

    let root_target = target(root);
    let parent_target = parent.map(target).unwrap_or_else(|| root_target.clone());

    CommentBuilder::new(content, parent_target)
        .root(root_target)
        .into_event_builder()
        .tags([Tag::coordinate(addr.clone().into(), None)])
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use nostr_sdk::prelude::*;
    use signed_core::GitEvent;

    use super::{RepoStore, comment_builder, patch_current_commit};

    #[test]
    fn parses_format_patch_header() {
        let patch = "From 1f6c0c5f3f1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a Mon Sep 17 00:00:00 2001\nFrom: A <a@b.c>\nSubject: [PATCH] fix\n\n---\n";
        assert_eq!(
            patch_current_commit(patch),
            Some("1f6c0c5f3f1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a")
        );
    }

    #[test]
    fn comment_builder_follows_nip22() {
        let keys = Keys::generate();
        let root = EventBuilder::new(Kind::GitIssue, "issue body")
            .finalize(&keys)
            .expect("signed event");
        let addr = signed_core::RepoAddr::new(root.pubkey, "my-repo");
        let relay = RelayUrl::parse("wss://relay.example.com").expect("valid relay URL");

        let event = comment_builder(&root, None, Some(&relay), &addr, "hi".into())
            .finalize(&keys)
            .expect("signed event");

        assert_eq!(event.kind, Kind::Comment);

        let kinds: Vec<&str> = event.tags.iter().map(Tag::kind).collect();
        for expected in ["E", "K", "P", "e", "k", "p", "a"] {
            assert!(kinds.contains(&expected), "missing {expected} tag");
        }

        let e = event.tags.iter().find(|t| t.kind() == "E").expect("E tag");
        let slice = e.as_slice();
        assert_eq!(slice[1], root.id.to_hex());
        assert_eq!(slice[2], relay.as_str());
        assert_eq!(slice[3], root.pubkey.to_hex());

        let e = event.tags.iter().find(|t| t.kind() == "e").expect("e tag");
        assert_eq!(e.as_slice()[1], root.id.to_hex());

        assert!(event.references_root(&root.id));
    }

    #[test]
    fn maintainer_filters_name_owner_and_maintainers() {
        let owner = Keys::generate().public_key();
        let maintainer = Keys::generate().public_key();
        let addr = signed_core::RepoAddr::new(owner, "my-repo");

        let filters = RepoStore::maintainer_filters(&addr, &[maintainer]);
        assert_eq!(filters.len(), 4);

        let expected = HashSet::from([owner, maintainer]);

        let named = |filter: &Filter| -> HashSet<PublicKey> {
            let authors = filter.authors.iter().flatten().copied();
            let p_tag = filter
                .generic_tags
                .get(&SingleLetterTag::LOWERCASE_P)
                .into_iter()
                .flatten()
                .filter_map(|value| PublicKey::from_hex(value).ok());
            authors.chain(p_tag).collect()
        };

        let announcement = &filters[0];
        assert_eq!(named(announcement), expected);
        assert!(
            announcement
                .generic_tags
                .contains_key(&SingleLetterTag::LOWERCASE_D)
        );

        for filter in &filters[1..3] {
            assert_eq!(named(filter), expected);
            assert!(
                filter
                    .generic_tags
                    .contains_key(&SingleLetterTag::LOWERCASE_A)
            );
        }

        let deletions = &filters[3];
        assert_eq!(
            deletions
                .authors
                .clone()
                .unwrap_or_default()
                .into_iter()
                .collect::<HashSet<_>>(),
            expected
        );
    }
}
