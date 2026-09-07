use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Error;
use bitcoin_hashes::sha1::Hash as Sha1Hash;
use gpui::{App, AppContext, AsyncApp, Context, SharedString, Subscription, Task, WeakEntity};
use nostr::event::IntoEventBuilder;
use nostr_sdk::prelude::*;
use signed_core::{
    Announcement, Deletions, RepoAddr, RepoStatus, filters, parse_state, pull_request_patch,
    pull_request_patches,
};

use crate::backend::{
    Backend, BackendEvent, grasp_base_url, grasp06_prs_url, pr_clone_urls, user_grasp_list_servers,
};
use crate::checkouts::CheckoutsStore;
use crate::git_store::GitStore;
use crate::refresh::{RefreshGate, RefreshRequest};
use crate::repo_list::RepoListStore;

/// Delay between a refresh request and the actual re-query.
const REFRESH_DEBOUNCE: Duration = Duration::from_millis(300);

/// Maximum size of one patch event.
///
/// NIP-34 suggests patches when each event is under 60kb.
const MAX_PATCH_EVENT_BYTES: usize = 60 * 1024;

/// Per-repository store.
///
/// Holds the announcement, state, issues, patches, PRs, comments and resolved statuses.
/// Always derived from the local database.
pub struct RepoStore {
    addr: RepoAddr,
    pub announcement: Option<Announcement>,
    /// Branch pointed to by `HEAD` in the latest state announcement.
    pub head: Option<String>,
    pub issues: Vec<Event>,
    pub patches: Vec<Event>,
    pub pull_requests: Vec<Event>,
    /// Comments on issues / PRs, oldest first.
    pub comments: Vec<Event>,
    /// Resolved status per root event, issue, patch or PR.
    status_by_root: HashMap<EventId, RepoStatus>,
    /// Open issue and root PR counts.
    /// Computed with [`Self::status_by_root`] on every refresh.
    open_issue_count: usize,
    open_pr_count: usize,
    /// Incremented on every applied refresh.
    ///
    /// Views key their derived-data caches to it instead of recomputing on every render.
    version: u64,
    /// Error of the last action initiated from this store, if any.
    pub last_error: Option<String>,
    /// Non-fatal warning of the last action, if any.
    ///
    /// Example, a PR published without its commit reaching a grasp server.
    pub last_warning: Option<String>,
    /// Warning of the last push that only some grasp servers accepted.
    ///
    /// The repository is out of sync on the rejected servers until it is republished.
    pub last_push_warning: Option<String>,
    /// A republish or a checkout push is in flight.
    ///
    /// Views show a spinner and disable their push triggers while it is set.
    pub pushing: bool,
    /// A clone-into-a-folder operation is in flight.
    ///
    /// Views show a spinner and disable the clone trigger while it is set.
    pub cloning: bool,
    /// Relays already asked to connect to, from this repository's NIP-34 `relays` tag.
    ///
    /// Avoids re-subscribing and re-fetching on every refresh.
    repo_relays: HashSet<RelayUrl>,
    /// Root events, issues, patches and PRs, already fetched per root.
    ///
    /// The per-root fetches cover NIP-22 comments and statuses without an `a` tag.
    root_fetches: HashSet<EventId>,
    /// Refresh coalescing, see [`RefreshGate`].
    refresh: RefreshGate,
    tasks: Vec<Task<Result<(), Error>>>,
    _subscription: Subscription,
}

impl RepoStore {
    pub fn new(addr: RepoAddr, announced_relays: Vec<RelayUrl>, cx: &mut Context<Self>) -> Self {
        let backend = Backend::global(cx);

        let subscription = cx.subscribe(&backend, |this, _backend, event, cx| {
            let relevant = match event {
                BackendEvent::NostrUpdate(update) => {
                    // Deletions may target any event of this repository.
                    let deletion =
                        update.kind == Kind::EventDeletion || update.kind == Kind::RequestToVanish;
                    let coordinate = update.coordinate.as_ref() == Some(&this.addr);
                    let author = update.author == this.addr.public_key;
                    let kind = update.kind == Kind::GitRepoAnnouncement;
                    // NIP-22 comments carry no `a` tag.
                    // Coordinate matching fails for them.
                    // Any comment may reference this repository's roots.
                    let comment = update.kind == Kind::Comment;
                    // Status events may omit their `a` tag, NIP-34.
                    // Any status event may reference a root of this repository.
                    let status = RepoStatus::from_kind(update.kind).is_some();

                    deletion || coordinate || (author && kind) || comment || status
                }
                BackendEvent::Published(event) => {
                    let kind = event.kind == Kind::GitRepoAnnouncement;
                    let author = event.pubkey == this.addr.public_key;
                    let coordinate = event.tags.coordinates().into_iter().any(|c| c == this.addr);
                    // Locally published deletions may target any event of this repository.
                    // Refresh so they take effect immediately, like relay deletions.
                    let deletion =
                        event.kind == Kind::EventDeletion || event.kind == Kind::RequestToVanish;

                    coordinate || (kind && author) || deletion
                }
                _ => false,
            };

            if relevant {
                this.refresh(cx);
            }
        });

        let mut store = Self {
            addr,
            announcement: None,
            head: None,
            issues: Vec::new(),
            patches: Vec::new(),
            pull_requests: Vec::new(),
            comments: Vec::new(),
            status_by_root: HashMap::new(),
            open_issue_count: 0,
            open_pr_count: 0,
            version: 0,
            last_error: None,
            last_warning: None,
            last_push_warning: None,
            pushing: false,
            cloning: false,
            repo_relays: HashSet::new(),
            root_fetches: HashSet::new(),
            refresh: RefreshGate::default(),
            _subscription: subscription,
            tasks: Vec::new(),
        };

        store.subscribe_remote(cx);
        // The announcement we opened the repo from may already list its relays.
        // Connect to them right away.
        // Do not wait for the bootstrap fetch to return the same event.
        store.connect_announced_relays(&announced_relays, cx);
        store.refresh(cx);
        store
    }

    /// Returns the repository's address.
    pub fn addr(&self) -> &RepoAddr {
        &self.addr
    }

    /// Returns the repository's name, or `Unknown` when not known.
    pub fn name(&self) -> SharedString {
        self.announcement
            .as_ref()
            .map_or(SharedString::default(), |a| {
                SharedString::from(a.name.as_deref().unwrap_or("Unknown"))
            })
    }

    /// Filters that make up a repository.
    ///
    /// Announcement, state, activity and deletions targeting it.
    fn repo_filters(addr: &RepoAddr) -> Vec<Filter> {
        let mut filters = vec![
            // Announcement and state share author and identifier.
            // They combine into one filter, one fewer negentropy reconciliation per relay.
            Filter::new()
                .kinds([Kind::GitRepoAnnouncement, Kind::RepoState])
                .author(addr.public_key)
                .identifier(addr.identifier.clone()),
            filters::activity(addr),
        ];
        // Deletion requests, NIP-09/62, must be known before any event is shown.
        filters.extend(filters::deletions_for_repo(addr));
        filters
    }

    /// Fetch this repository's events from the relays in its NIP-34 `relays` tag.
    fn connect_announced_relays(&mut self, relays: &[RelayUrl], cx: &mut Context<Self>) {
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
        let addr = self.addr.clone();

        backend.update(cx, |backend, cx| {
            backend.connect_repo_relays(new, Self::repo_filters(&addr), cx);
        });
    }

    /// Fetch this repository's events from the bootstrap relays.
    fn subscribe_remote(&mut self, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let addr = self.addr.clone();

        backend.update(cx, |backend, cx| {
            backend.subscribe_bootstrap(Self::repo_filters(&addr), cx);
        });
    }

    /// Re-query the local database and update all fields.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refresh.request() != RefreshRequest::Schedule {
            return;
        }

        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH_DEBOUNCE).await;

            this.update(cx, |this, cx| this.run_refresh(cx))
        });

        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(task);
    }

    fn run_refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh.begin();

        let backend = Backend::global(cx);
        let client = backend.read(cx).client();
        let addr = self.addr.clone();

        let work = cx.background_spawn(async move {
            let (announcements, states, activity, deletion_events) = async {
                let db = client.database();
                let announcements = db.query(filters::announcement(&addr)).await?;
                let states = db.query(filters::state(&addr)).await?;
                let activity = db.query(filters::activity(&addr)).await?;
                let deletion_events = db.query(filters::deletions()).await?;

                Ok::<_, Error>((announcements, states, activity, deletion_events))
            }
            .await?;

            let deletions = Deletions::from_events(deletion_events);

            // Parse and sort off the main thread.
            // Only plain data crosses back into the entity.
            let all_announcements = announcements
                .into_iter()
                .filter(|e| !deletions.is_deleted(e));
            let announcement = latest(all_announcements)
                .as_ref()
                .and_then(Announcement::from_event);

            let all_states = states.into_iter().filter(|e| !deletions.is_deleted(e));
            let state = latest(all_states).map(|state| parse_state(&state));

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

            // NIP-22 comments reference their root via an `E` or `e` tag.
            // Not the repository's `a` tag, so query them by the root events.
            let mut seen_comments: HashSet<EventId> = comments.iter().map(|e| e.id).collect();
            let db = client.database();

            let roots = issues
                .iter()
                .chain(&patches)
                .chain(&pull_requests)
                .map(|e| e.id);

            for filter in filters::comments_for(roots) {
                for event in db.query(filter).await? {
                    if seen_comments.insert(event.id) {
                        comments.push(event);
                    }
                }
            }

            // Status events may omit their `a` tag.
            // Query them by the root events they reference too.
            let mut seen_statuses: HashSet<EventId> = statuses.iter().map(|e| e.id).collect();
            let db = client.database();

            let roots = issues
                .iter()
                .chain(&patches)
                .chain(&pull_requests)
                .map(|e| e.id);

            for root in roots {
                for event in db.query(filters::statuses_for([root])).await? {
                    if seen_statuses.insert(event.id) {
                        statuses.push(event);
                    }
                }
            }

            // Cover notes, 1624, and label events, 1985, carry no `a` tag.
            // Query them per root like comments and statuses.
            // The events are only stored for interop and nothing displays them.
            sort_newest_first(&mut issues);
            sort_newest_first(&mut patches);
            sort_newest_first(&mut pull_requests);
            sort_oldest_first(&mut comments);

            // Resolve every root's status once here.
            // Render paths do HashMap lookups instead of per-root status scans.
            // Those scans are quadratic, with an allocation per pair.
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

        self.tasks.retain(|task| !task.is_ready());

        self.tasks.push(cx.spawn(async move |this, cx| {
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
                        this.refresh.abort();
                        this.last_error = Some(e.to_string());
                        cx.notify();
                    });
                }
            };

            let again = this.update(cx, |this, cx| {
                this.announcement = announcement;

                // The announcement may list relays for this repository's activity.
                // Connect to any we have not fetched from yet.
                let relays = this
                    .announcement
                    .as_ref()
                    .map(|a| a.relays.clone())
                    .unwrap_or_default();
                this.connect_announced_relays(&relays, cx);

                if let Some((_, head)) = state {
                    this.head = head;
                }

                this.issues = issues;
                this.patches = patches;
                this.pull_requests = pull_requests;
                this.comments = comments;
                this.status_by_root = status_by_root;
                this.open_issue_count = open_issue_count;
                this.open_pr_count = open_pr_count;
                this.version = this.version.wrapping_add(1);

                // Comments and statuses without an `a` tag.
                // None are addressed to the repository.
                // Fetch them by the root events they reference.
                // Use the bootstrap relays and the relays this repository announced.
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
                    // Batch the per-root filters.
                    // One filter per root costs a negentropy reconciliation per relay.
                    let mut root_filters = filters::comments_for(new_roots.clone());
                    root_filters.push(filters::statuses_for(new_roots.iter().copied()));

                    let announced: Vec<RelayUrl> = this.repo_relays.iter().cloned().collect();
                    let backend = Backend::global(cx);
                    backend.update(cx, |backend, cx| {
                        backend.subscribe_bootstrap(root_filters.clone(), cx);
                        backend.connect_repo_relays(announced, root_filters, cx);
                    });
                }

                cx.notify();

                this.refresh.finish()
            })?;

            // Requests that arrived while the refresh was running.
            // They are coalesced into one follow-up refresh.
            if again {
                this.update(cx, |this, cx| this.refresh(cx))?;
            }

            Ok(())
        }));
    }

    /// Resolve the status of a root event, an issue, patch or PR, per NIP-34.
    pub fn status_of(&self, root: &Event) -> RepoStatus {
        status_of(&self.status_by_root, root)
    }

    /// Refresh generation, incremented on every applied refresh.
    /// Views use it to key their derived-data caches.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Number of open issues.
    ///
    /// Issues whose resolved status is [`RepoStatus::Open`].
    /// Issues without status events default to open.
    pub fn issue_count(&self) -> usize {
        self.open_issue_count
    }

    /// Number of open pull requests.
    ///
    /// Only root PR events count, PR updates do not.
    /// They must resolve to [`RepoStatus::Open`].
    pub fn pull_request_count(&self) -> usize {
        self.open_pr_count
    }

    /// Whether `user` is the author or owner of this repository.
    ///
    /// The author is the public key of the repository address.
    /// Only the author may manage pull requests, close, reopen or merge.
    pub fn is_author(&self, user: &PublicKey) -> bool {
        &self.addr.public_key == user
    }

    /// Open an issue on this repository.
    pub fn open_issue(&mut self, subject: Option<String>, content: String, cx: &mut Context<Self>) {
        let builder = GitIssue {
            repository: self.addr.clone(),
            content,
            subject,
            labels: Vec::new(),
        }
        .into_event_builder();

        self.send(builder, cx);
    }

    /// Comments on a root event, an issue or PR, oldest first.
    pub fn comments_of(&self, root: &EventId) -> impl Iterator<Item = &Event> {
        self.comments
            .iter()
            .filter(move |e| signed_core::references_root(e, root))
    }

    /// Comment on a root event, an issue or PR, per NIP-34, kind 1111.
    pub fn comment(&mut self, root: &Event, content: String, cx: &mut Context<Self>) {
        self.reply(root, None, content, cx);
    }

    /// Reply to `parent`, a comment on `root`, with a NIP-22 threaded comment.
    ///
    /// `None` publishes a top-level comment on the root itself.
    pub fn reply(
        &mut self,
        root: &Event,
        parent: Option<&Event>,
        content: String,
        cx: &mut Context<Self>,
    ) {
        let relay_hint = self
            .announcement
            .as_ref()
            .and_then(|a| a.relays.first())
            .cloned();

        self.send(
            comment_builder(root, parent, relay_hint.as_ref(), &self.addr, content),
            cx,
        );
    }

    /// Open a pull request on this repository.
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

        let series: Vec<String> = signed_git::split_patch_series(&patch)
            .into_iter()
            .map(str::to_owned)
            .collect();

        if let Some(oversized) = series
            .iter()
            .find(|part| part.len() > MAX_PATCH_EVENT_BYTES)
        {
            self.last_error = Some(format!(
                "patch too large ({} bytes; NIP-34 suggests keeping each patch under {} bytes)",
                oversized.len(),
                MAX_PATCH_EVENT_BYTES
            ));
            cx.notify();
            return;
        }

        // The tip of the series is its last commit.
        // `git format-patch` orders patches oldest first.
        let Some(current_commit) = series
            .last()
            .and_then(|part| patch_current_commit(part))
            .and_then(|hex| hex.parse::<Sha1Hash>().ok())
        else {
            self.last_error = Some(
                "Patch must be `git format-patch` output with a `From <commit-id>` header".into(),
            );
            cx.notify();
            return;
        };

        let backend = Backend::global(cx);
        let signer = backend.read(cx).signer();

        let Some(user) = backend.read(cx).current_user() else {
            self.last_error = Some("Sign in to open a pull request".into());
            cx.notify();
            return;
        };
        // The author's npub names their GRASP-06 namespace, `/prs/...`.
        let author_npub = user.to_bech32().unwrap();

        let addr = self.addr.clone();
        let owner = self.addr.public_key;
        let euc = self.announcement.as_ref().and_then(|a| a.euc.clone());
        let repo_id = addr.identifier.clone();
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

        self.tasks.push(cx.spawn(async move |this, cx| {
            // The PR references the root patch event.
            // Viewers can then find the patch without carrying it inline.
            let root_patch = match publish_patch_series(
                &this,
                cx,
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

            // GRASP-06 pushes the tip to the author's own grasp servers.
            // The path is `/prs/<author-npub>/<repo-id>.git`.
            // Contributing to another project never depends on that project's servers.
            // Resolve the servers from the author's latest kind-10317 grasp list.
            // The settings defaults stand in when no list is published or the query fails.
            let author_servers = {
                let query = this.update(cx, |_this, cx| {
                    let client = Backend::global(cx).read(cx).client();
                    user_grasp_list_servers(client, user)
                })?;
                match cx.background_spawn(query).await {
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
                // NIP-34 PRs carry at least one clone URL.
                // The tip commit is downloadable from it.
                // The author's `/prs/` URLs come first.
                // They are author-controlled and most likely alive.
                // The announced mirrors follow.
                // The list is fixed before signing.
                // The pushed ref name embeds the event id.
                // Every candidate URL is listed up front.
                // Dead URLs are inert, the linked patch stays the source of truth.
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
                    repository: this.addr.clone(),
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

            // Sign before publishing.
            // The tip is pushed to the grasp servers under `refs/nostr/<event-id>`.
            // Nak's convention, readers fetch that ref for the commit behind the `c` tag.
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
                        // Author servers first, then the announced base grasp servers.
                        // The extra targets are best-effort redundancy.
                        let targets: Vec<(String, String)> = author_targets
                            .into_iter()
                            .chain(base_targets)
                            .collect();
                        async move {
                            let mut failures = Vec::new();
                            let mut pushed = 0;
                            for (url, label) in &targets {
                                match signed_git::push_commit_ref(
                                    &path, url, &tip, &reference,
                                ) {
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

            let publish_task = this.update(cx, |_this, cx| {
                let backend = Backend::global(cx);
                backend.update(cx, |backend, cx| backend.publish_event(event, cx))
            })?;

            let pr_event = match publish_task.await {
                Ok(event) => event,
                Err(e) => {
                    return this.update(cx, |this, cx| {
                        this.last_error = Some(e.to_string());
                        cx.notify();
                    });
                }
            };

            // A draft PR carries a kind-1633 status event, NIP-34.
            // Publish it right after the PR event so viewers never show it open.
            if draft {
                this.update(cx, |this, cx| {
                    this.set_status(&pr_event, RepoStatus::Draft, cx);
                })?;
            }

            Ok(())
        }));
    }

    /// Update a pull request.
    ///
    /// Other authors must open a new PR.
    pub fn update_pull_request(&mut self, root: &Event, patch: String, cx: &mut Context<Self>) {
        self.last_error = None;
        self.last_warning = None;

        let backend = Backend::global(cx);

        let Some(user) = backend.read(cx).current_user() else {
            self.last_error = Some("Sign in to update the pull request".into());
            cx.notify();
            return;
        };

        if user != root.pubkey {
            self.last_error = Some("Only the pull request author can update it".into());
            cx.notify();
            return;
        }

        let series: Vec<String> = signed_git::split_patch_series(&patch)
            .into_iter()
            .map(str::to_owned)
            .collect();
        if let Some(oversized) = series
            .iter()
            .find(|part| part.len() > MAX_PATCH_EVENT_BYTES)
        {
            self.last_error = Some(format!(
                "patch too large ({} bytes; NIP-34 suggests keeping each patch under {} bytes)",
                oversized.len(),
                MAX_PATCH_EVENT_BYTES
            ));
            cx.notify();
            return;
        }

        // The new tip of the PR is the last commit of the series.
        let Some(current_commit) = series
            .last()
            .and_then(|part| patch_current_commit(part))
            .and_then(|hex| hex.parse::<Sha1Hash>().ok())
        else {
            self.last_error = Some(
                "Patch must be `git format-patch` output with a `From <commit-id>` header".into(),
            );
            cx.notify();
            return;
        };

        // The first revision patch replies to the original root patch, NIP-34.
        // Use the PR's `e` tag, or the oldest patch of the linked set if the PR has none.
        let root_patch_id = root.tags.event_ids().next().or_else(|| {
            pull_request_patches(root, self.patches.iter())
                .first()
                .map(|p| p.id)
        });

        let addr = self.addr.clone();
        let owner = self.addr.public_key;
        let euc = self.announcement.as_ref().and_then(|a| a.euc.clone());
        let root = root.clone();
        let clone: Vec<Url> = self
            .announcement
            .as_ref()
            .map(|a| a.clone.clone())
            .unwrap_or_default();

        self.tasks.push(cx.spawn(async move |this, cx| {
            if let Err(e) = publish_patch_series(
                &this,
                cx,
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

            let update_task = this.update(cx, |this, cx| {
                let builder = GitPullRequestUpdate {
                    repository: this.addr.clone(),
                    pull_request_event: root.id,
                    pull_request_author: root.pubkey,
                    current_commit,
                    clone: clone.clone(),
                    merge_base: None,
                }
                .into_event_builder();

                // The `r` EUC tag lets clients subscribe to all PR updates.
                // The SDK builder omits it.
                let builder = match euc.as_deref() {
                    Some(euc) => builder.tag(Tag::parse(["r", euc]).expect("valid r tag")),
                    None => builder,
                };

                let backend = Backend::global(cx);
                backend.update(cx, |backend, cx| backend.send(builder, cx))
            })?;

            if let Err(e) = update_task.await {
                return this.update(cx, |this, cx| {
                    this.last_error = Some(e.to_string());
                    cx.notify();
                });
            }

            Ok(())
        }));
    }

    /// Set the status of a root event.
    ///
    /// Only the root author or a maintainer may set it, per NIP-34.
    pub fn set_status(&mut self, root: &Event, status: RepoStatus, cx: &mut Context<Self>) {
        self.last_error = None;

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
            Tag::public_key(self.addr.public_key),
            Tag::public_key(root.pubkey),
            Tag::coordinate(self.addr.clone(), None),
        ]);

        self.send(builder, cx);
    }

    /// Merge a pull request.
    pub fn merge_pull_request(&mut self, root: &Event, cx: &mut Context<Self>) {
        self.last_error = None;
        self.last_warning = None;

        let is_author = Backend::global(cx)
            .read(cx)
            .current_user()
            .is_some_and(|user| self.is_author(&user));
        if !is_author {
            self.last_error = Some("Only the repository author can merge pull requests".into());
            return;
        }

        let cache = GitStore::global(cx).cache().clone();
        let addr = self.addr.clone();

        let clone_urls: Vec<String> = self
            .announcement
            .as_ref()
            .map(|a| a.clone.iter().map(ToString::to_string).collect())
            .unwrap_or_default();

        let patch = pull_request_patch(root, self.patches.iter());

        // The applied patch events, for the status tags below.
        let patches: Vec<Event> = pull_request_patches(root, self.patches.iter())
            .into_iter()
            .cloned()
            .collect();

        let relay_hint = self
            .announcement
            .as_ref()
            .and_then(|a| a.relays.first())
            .map(ToString::to_string)
            .unwrap_or_default();

        let euc = self.announcement.as_ref().and_then(|a| a.euc.clone());
        let root = root.clone();

        let apply = cx.background_spawn(async move {
            let repo = cache.ensure_clone(&addr, &clone_urls)?;
            let workdir = repo
                .workdir()
                .ok_or_else(|| anyhow::anyhow!("repository has no worktree"))?
                .to_path_buf();
            // The commits the apply created.
            // Everything between the previous HEAD and the new one, oldest first.
            let previous = signed_git::head_commit_id(&workdir)?;
            signed_git::apply_patch(&workdir, &patch)?;
            let applied = signed_git::commits_since(&workdir, previous.as_deref())?;
            Ok::<_, Error>(applied)
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            match apply.await {
                Ok(applied) => {
                    this.update(cx, |this, cx| {
                        this.publish_applied_status(
                            &root,
                            &patches,
                            &applied,
                            &relay_hint,
                            euc.as_deref(),
                            cx,
                        );
                    })?;
                }
                Err(e) => {
                    this.update(cx, |this, cx| {
                        this.last_error = Some(e.to_string());
                        cx.notify();
                    })?;
                }
            }
            Ok(())
        }));
    }

    /// The latest announcement of this repository,
    /// for operations that need its clone URLs and relays.
    fn action_announcement(&self, cx: &App) -> Option<Announcement> {
        self.announcement.clone().or_else(|| {
            RepoListStore::global(cx)
                .read(cx)
                .announcements
                .iter()
                .find(|announcement| announcement.addr() == self.addr)
                .cloned()
        })
    }

    /// Re-push the repository's refs to its announced grasp servers, republish.
    pub fn push_repository(&mut self, cx: &mut Context<Self>) -> Task<Result<(), Error>> {
        if self.pushing {
            return Task::ready(Err(anyhow::anyhow!(
                "A push to this repository is already in progress"
            )));
        }
        let Some(announcement) = self.action_announcement(cx) else {
            return self.action_error("Repository announcement is not loaded yet", cx);
        };

        self.pushing = true;
        self.last_error = None;
        self.last_push_warning = None;
        cx.notify();

        let backend = Backend::global(cx);
        let push = backend.update(cx, |backend, cx| backend.push_repository(announcement, cx));

        cx.spawn(async move |this, cx| {
            let result = push.await;

            this.update(cx, |this, cx| {
                this.pushing = false;

                match &result {
                    Ok(outcome) => {
                        this.last_error = None;
                        // A push only some grasp servers accepted is a warning:
                        // the repo is out of sync on the rest until it is republished.
                        this.last_push_warning = outcome.partial_warning();
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

    /// Push the unpushed commits of the local checkout at `path`.
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

        let Some(announcement) = self.action_announcement(cx) else {
            return self.action_error("Repository announcement is not loaded yet", cx);
        };

        // The state event's `HEAD` stays the announced default branch.
        // The checkout may be on a side branch.
        let head = self.head.clone();
        let addr = self.addr.clone();

        self.pushing = true;
        self.last_error = None;
        self.last_push_warning = None;
        cx.notify();

        let checkouts = CheckoutsStore::global(cx);
        let backend = Backend::global(cx);
        let push = backend.update(cx, |backend, cx| {
            backend.push_checkout(announcement, path.clone(), head, cx)
        });

        cx.spawn(async move |this, cx| {
            let result = push.await;

            this.update(cx, |this, cx| {
                this.pushing = false;

                match &result {
                    Ok(outcome) => {
                        this.last_error = None;
                        // A push only some grasp servers accepted is a warning:
                        // the repo is out of sync on the rest until it is republished.
                        this.last_push_warning = outcome.partial_warning();
                        // The remote moved, so recompute the ready-to-push statuses.
                        checkouts.update(cx, |store, cx| {
                            store.checkout_pushed(&addr, &path, cx);
                        });
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

    /// Delete the repository from nostr, announcement, state and activity.
    ///
    /// Only the repository owner may delete it. The lists update when the
    /// deletion events arrive.
    pub fn delete_repository(&mut self, cx: &mut Context<Self>) -> Task<Result<(), Error>> {
        let addr = self.addr.clone();
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

    /// Clone the repository into `destination`, a user-chosen folder outside
    /// the cache, and remember the clone as a checkout of this repository.
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
        let Some(announcement) = self.action_announcement(cx) else {
            return self.action_error("Repository announcement is not loaded yet", cx);
        };

        let clone_urls: Vec<String> = announcement.clone.iter().map(ToString::to_string).collect();
        let addr = self.addr.clone();

        self.cloning = true;
        self.last_error = None;
        cx.notify();

        let clone = {
            let destination = destination.clone();
            cx.background_spawn(async move { signed_git::clone_repo(&clone_urls, &destination) })
        };

        cx.spawn(async move |this, cx| {
            let result = clone.await;

            this.update(cx, |this, cx| {
                this.cloning = false;

                match &result {
                    Ok(()) => {
                        // Remember the clone as a checkout of this repository.
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

    /// Fail an operation whose announcement is not loaded yet.
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

    /// Publish a kind-1631 Applied status event for `root` after a merge.
    fn publish_applied_status(
        &mut self,
        root: &Event,
        patches: &[Event],
        applied: &[String],
        relay_hint: &str,
        euc: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let mut tags = vec![
            Tag::parse(["e", &root.id.to_hex(), "", "root"]).expect("valid root tag"),
            Tag::public_key(self.addr.public_key),
            Tag::public_key(root.pubkey),
            Tag::coordinate(self.addr.clone(), None),
        ];

        if let Some(euc) = euc
            && let Ok(tag) = Tag::parse(["r", euc])
        {
            tags.push(tag);
        }

        // Tag each applied patch event.
        // `q` per event, `e` reply for events beyond the root, chain parts and revisions.
        // Their statuses then resolve to Applied too.
        for (ix, patch) in patches.iter().enumerate() {
            if let Ok(tag) =
                Tag::parse(["q", &patch.id.to_hex(), relay_hint, &patch.pubkey.to_hex()])
            {
                tags.push(tag);
            }
            if ix > 0
                && let Ok(tag) = Tag::parse(["e", &patch.id.to_hex(), "", "reply"])
            {
                tags.push(tag);
            }
        }

        // The commits `git am` created on top of the previous HEAD.
        if !applied.is_empty() {
            let mut applied_tag = vec!["applied-as-commits".to_string()];
            applied_tag.extend(applied.iter().cloned());
            if let Ok(tag) = Tag::parse(applied_tag) {
                tags.push(tag);
            }
            for commit in applied {
                if let Ok(tag) = Tag::parse(["r", commit]) {
                    tags.push(tag);
                }
            }
        }

        self.send(EventBuilder::new(Kind::GitStatusApplied, "").tags(tags), cx);
    }

    fn send(&mut self, builder: EventBuilder, cx: &mut Context<Self>) {
        self.last_error = None;

        let backend = Backend::global(cx);
        let task = backend.update(cx, |backend, cx| backend.send(builder, cx));

        self.tasks.push(cx.spawn(async move |this, cx| {
            if let Err(e) = task.await {
                this.update(cx, |this, cx| {
                    this.last_error = Some(e.to_string());
                    cx.notify();
                })?;
            }
            Ok(())
        }));
    }
}

fn latest<I>(events: I) -> Option<Event>
where
    I: IntoIterator<Item = Event>,
{
    events.into_iter().max_by_key(|e| e.created_at)
}

/// Status of `root` from the precomputed map.
fn status_of(status_by_root: &HashMap<EventId, RepoStatus>, root: &Event) -> RepoStatus {
    status_by_root
        .get(&root.id)
        .copied()
        .unwrap_or(RepoStatus::Open)
}

/// Resolve every root event's status in one pass.
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
                signed_core::resolve_status(events.iter().copied(), &root.pubkey, maintainers);
            (root.id, status)
        })
        .collect()
}

fn sort_newest_first(events: &mut [Event]) {
    events.sort_by_key(|e| std::cmp::Reverse(e.created_at));
}

fn sort_oldest_first(events: &mut [Event]) {
    events.sort_by_key(|e| e.created_at);
}

/// The proposed commit of a `git format-patch` output.
/// It is the `From <commit>` header on the first line.
fn patch_current_commit(patch: &str) -> Option<&str> {
    let line = patch.lines().next()?;
    let hex = line.strip_prefix("From ")?;
    hex.split_whitespace().next().filter(|hex| hex.len() == 40)
}

/// Publish a `git format-patch` series as chained kind-1617 events.
///
/// Returns the root event, the one a PR references.
#[allow(clippy::too_many_arguments)]
async fn publish_patch_series(
    this: &WeakEntity<RepoStore>,
    cx: &mut AsyncApp,
    addr: &RepoAddr,
    owner: PublicKey,
    euc: Option<&str>,
    series: &[String],
    first_marker: &str,
    reply_to: Option<EventId>,
) -> Result<Event, Error> {
    let mut root: Option<Event> = None;
    let mut previous = reply_to;

    for (ix, part) in series.iter().enumerate() {
        let Some(commit) = patch_current_commit(part).filter(|hex| hex.len() == 40) else {
            return Err(anyhow::anyhow!(
                "patch {} of the series has no `From <commit-id>` header",
                ix + 1
            ));
        };

        let mut tags = vec![Tag::coordinate(addr.clone(), None), Tag::public_key(owner)];
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

        let task = this.update(cx, |_this, cx| {
            let backend = Backend::global(cx);
            backend.update(cx, |backend, cx| backend.send(builder, cx))
        })?;
        let event = task.await?;

        if root.is_none() {
            root = Some(event.clone());
        }
        previous = Some(event.id);
    }

    root.ok_or_else(|| anyhow::anyhow!("patch series is empty"))
}

/// Build a NIP-22 kind-1111 comment.
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
        .tags([Tag::coordinate(addr.clone(), None)])
}

#[cfg(test)]
mod tests {
    use nostr_sdk::prelude::*;

    use super::{comment_builder, patch_current_commit};

    #[test]
    fn parses_format_patch_header() {
        let patch = "From 1f6c0c5f3f1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a Mon Sep 17 00:00:00 2001\nFrom: A <a@b.c>\nSubject: [PATCH] fix\n\n---\n";
        assert_eq!(
            patch_current_commit(patch),
            Some("1f6c0c5f3f1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a")
        );
    }

    #[test]
    fn no_commit_without_header() {
        assert_eq!(patch_current_commit(""), None);
        assert_eq!(patch_current_commit("Subject: [PATCH] x\n\n---\n"), None);
        assert_eq!(patch_current_commit("From short\n"), None);
    }

    #[test]
    fn comment_builder_follows_nip22() {
        let keys = Keys::generate();
        let root = EventBuilder::new(Kind::GitIssue, "issue body")
            .finalize(&keys)
            .expect("signed event");
        let addr = Coordinate::new(Kind::GitRepoAnnouncement, root.pubkey).identifier("my-repo");
        let relay = RelayUrl::parse("wss://relay.example.com").expect("valid relay URL");

        let event = comment_builder(&root, None, Some(&relay), &addr, "hi".into())
            .finalize(&keys)
            .expect("signed event");

        assert_eq!(event.kind, Kind::Comment);

        let kinds: Vec<&str> = event.tags.iter().map(Tag::kind).collect();
        for expected in ["E", "K", "P", "e", "k", "p", "a"] {
            assert!(kinds.contains(&expected), "missing {expected} tag");
        }

        // The uppercase `E` tag scopes the root, with its id, relay hint and author.
        let e = event.tags.iter().find(|t| t.kind() == "E").expect("E tag");
        let slice = e.as_slice();
        assert_eq!(slice[1], root.id.to_hex());
        assert_eq!(slice[2], relay.as_str());
        assert_eq!(slice[3], root.pubkey.to_hex());

        // The lowercase `e` tag references the parent.
        // For a top-level comment the parent is the root itself.
        let e = event.tags.iter().find(|t| t.kind() == "e").expect("e tag");
        assert_eq!(e.as_slice()[1], root.id.to_hex());

        // Signed's own `references_root` must keep matching the comment.
        assert!(signed_core::references_root(&event, &root.id));
    }

    #[test]
    fn comment_builder_replies_nest_under_the_parent() {
        let keys = Keys::generate();
        let root = EventBuilder::new(Kind::GitIssue, "issue body")
            .finalize(&keys)
            .expect("signed event");
        let parent = EventBuilder::new(Kind::Comment, "first comment")
            .finalize(&keys)
            .expect("signed event");
        let addr = Coordinate::new(Kind::GitRepoAnnouncement, root.pubkey).identifier("my-repo");

        let event = comment_builder(&root, Some(&parent), None, &addr, "reply".into())
            .finalize(&keys)
            .expect("signed event");

        // The uppercase `E` tag still scopes the root event.
        // The lowercase `e` tag references the parent comment.
        let root_ref = event.tags.iter().find(|t| t.kind() == "E").expect("E tag");
        let parent_ref = event.tags.iter().find(|t| t.kind() == "e").expect("e tag");
        assert_eq!(root_ref.as_slice()[1], root.id.to_hex());
        assert_eq!(parent_ref.as_slice()[1], parent.id.to_hex());

        // The reply still threads under the root for Signed's own display.
        assert!(signed_core::references_root(&event, &root.id));
    }
}
