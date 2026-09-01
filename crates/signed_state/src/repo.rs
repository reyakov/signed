use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::Error;
use gpui::{AppContext, Context, Subscription, Task};
use nostr::event::IntoEventBuilder;
use nostr_sdk::prelude::*;
use signed_core::{
    Announcement, COVER_NOTE_KIND, Deletions, RepoAddr, RepoStatus, build_state, cover_note,
    filters, labels_and_subject, parse_state, pull_request_patch, subject_override,
};

use crate::backend::{Backend, BackendEvent};
use crate::git_store::GitStore;

/// Delay between a refresh request and the actual re-query, so bursts of
/// events (e.g. per-event `NostrUpdate`s) collapse into one query.
const REFRESH_DEBOUNCE: Duration = Duration::from_millis(300);

/// Per-repository store: announcement, state, issues, patches, PRs,
/// comments and their resolved statuses. Always derived from the local
/// database.
pub struct RepoStore {
    addr: RepoAddr,
    pub announcement: Option<Announcement>,
    /// `(refname, commit-id)` pairs from the latest state announcement.
    pub refs: Vec<(String, String)>,
    /// Branch pointed to by `HEAD` in the latest state announcement.
    pub head: Option<String>,
    pub issues: Vec<Event>,
    pub patches: Vec<Event>,
    pub pull_requests: Vec<Event>,
    /// Comments on issues / PRs, oldest first.
    pub comments: Vec<Event>,
    /// Resolved status per root event (issue / patch / PR), recomputed on
    /// every refresh so render paths are HashMap lookups instead of
    /// scanning all status events per root.
    status_by_root: HashMap<EventId, RepoStatus>,
    /// Open issue / root PR counts, computed with [`Self::status_by_root`]
    /// on every refresh.
    open_issue_count: usize,
    open_pr_count: usize,
    /// Kind-1624 cover notes and kind-1985 label events referencing this
    /// repository's roots (ngit / GitWorkshop extensions).
    cover_notes: Vec<Event>,
    labels: Vec<Event>,
    /// Incremented on every applied refresh; views key their derived-data
    /// caches to it instead of recomputing on every render.
    version: u64,
    /// Error of the last action initiated from this store, if any.
    pub last_error: Option<String>,
    /// Relays announced by this repository (NIP-34 `relays` tag) that we
    /// have already been asked to connect to and fetch from, to avoid
    /// re-subscribing on every refresh.
    repo_relays: HashSet<RelayUrl>,
    /// Root events (issues, patches, PRs) for which the per-root fetches
    /// (NIP-22 comments, statuses without an `a` tag, cover notes and
    /// labels) have already been requested, to avoid re-fetching on every
    /// refresh.
    root_fetches: HashSet<EventId>,
    refreshing: bool,
    refresh_dirty: bool,
    /// A refresh is waiting out [`REFRESH_DEBOUNCE`].
    debouncing: bool,
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
                    // NIP-22 comments carry no `a` tag, so they can't be
                    // matched by coordinate; any comment may reference this
                    // repository's roots.
                    let comment = update.kind == Kind::Comment;
                    // Status events may omit their `a` tag (NIP-34), so any
                    // status event may reference a root of this repository.
                    let status = RepoStatus::from_kind(update.kind).is_some();
                    // Cover notes and labels carry no `a` tag either.
                    let annotation = update.kind == COVER_NOTE_KIND || update.kind == Kind::Label;

                    deletion || coordinate || (author && kind) || comment || status || annotation
                }
                BackendEvent::Published(event) => {
                    let kind = event.kind == Kind::GitRepoAnnouncement;
                    let author = event.pubkey == this.addr.public_key;
                    let coordinate = event.tags.coordinates().into_iter().any(|c| c == this.addr);
                    // Locally published deletions may target any event of
                    // this repository; refresh so they take effect
                    // immediately, like relay deletions.
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
            refs: Vec::new(),
            head: None,
            issues: Vec::new(),
            patches: Vec::new(),
            pull_requests: Vec::new(),
            comments: Vec::new(),
            status_by_root: HashMap::new(),
            open_issue_count: 0,
            open_pr_count: 0,
            cover_notes: Vec::new(),
            labels: Vec::new(),
            version: 0,
            last_error: None,
            repo_relays: HashSet::new(),
            root_fetches: HashSet::new(),
            refreshing: false,
            refresh_dirty: false,
            debouncing: false,
            _subscription: subscription,
            tasks: Vec::new(),
        };

        store.subscribe_remote(cx);
        // The announcement we opened the repo from may already list its
        // relays; connect to them right away instead of waiting for the
        // bootstrap fetch to return the same event.
        store.connect_announced_relays(&announced_relays, cx);
        store.refresh(cx);
        store
    }

    pub fn addr(&self) -> &RepoAddr {
        &self.addr
    }

    /// Filters that make up a repository: announcement, state, activity and
    /// deletions targeting it.
    fn repo_filters(addr: &RepoAddr) -> Vec<Filter> {
        let mut filters = vec![
            // Announcement and state share author and identifier, so they
            // combine into one filter: one fewer negentropy reconciliation
            // per relay when fetching from the repo's announced relays.
            Filter::new()
                .kinds([Kind::GitRepoAnnouncement, Kind::RepoState])
                .author(addr.public_key)
                .identifier(addr.identifier.clone()),
            filters::activity(addr),
        ];
        // Deletion requests (NIP-09/62) must be known before any event of
        // this repository can be shown.
        filters.extend(filters::deletions_for_repo(addr));
        filters
    }

    /// Fetch this repository's events from the relays announced in its
    /// NIP-34 `relays` tag. Deduplicated: each relay is only contacted once
    /// per store, so refreshes after the first are no-ops unless the
    /// announcement lists new relays.
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

    /// Fetch this repository's events from the bootstrap relays (one-shot,
    /// auto-closing subscription).
    fn subscribe_remote(&mut self, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let addr = self.addr.clone();

        backend.update(cx, |backend, cx| {
            backend.subscribe_bootstrap(Self::repo_filters(&addr), cx);
        });
    }

    /// Re-query the local database and update all fields.
    ///
    /// Debounced: a short delay collapses bursts of requests (e.g. per-event
    /// `NostrUpdate`s), and requests that arrive while a query is running are
    /// folded into one follow-up query. The query and processing run on a
    /// background thread; only the results are applied on the main thread.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            self.refresh_dirty = true;
            return;
        }
        if self.debouncing {
            return;
        }
        self.debouncing = true;

        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH_DEBOUNCE).await;

            this.update(cx, |this, cx| {
                this.debouncing = false;
                this.run_refresh(cx);
            })
        });

        self.tasks.retain(|task| !task.is_ready());
        self.tasks.push(task);
    }

    /// One query + apply cycle (debounced entry point).
    fn run_refresh(&mut self, cx: &mut Context<Self>) {
        self.refreshing = true;

        let client = Backend::global(cx).read(cx).client();
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

            // Parse and sort off the main thread; only plain data
            // crosses back into the entity.
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
            let (mut cover_notes, mut labels): (Vec<Event>, Vec<Event>) = (Vec::new(), Vec::new());

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

            // NIP-22 comments reference their root via an `E`/`e` tag rather
            // than the repository's `a` tag, so query them by the root events
            // of this repository.
            let db = client.database();
            let mut seen_comments: HashSet<EventId> = comments.iter().map(|e| e.id).collect();
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

            // Status events may omit their `a` tag (NIP-34 makes it
            // optional), so also query them by the root events they
            // reference.
            let db = client.database();
            let mut seen_statuses: HashSet<EventId> = statuses.iter().map(|e| e.id).collect();
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

            // Cover notes (1624) and label events (1985) reference their
            // target via an `e` tag, so query them per root like comments
            // and statuses.
            let db = client.database();
            let mut seen_cover_notes: HashSet<EventId> = cover_notes.iter().map(|e| e.id).collect();
            let mut seen_labels: HashSet<EventId> = labels.iter().map(|e| e.id).collect();
            let roots = issues
                .iter()
                .chain(&patches)
                .chain(&pull_requests)
                .map(|e| e.id);
            for root in roots {
                for event in db.query(filters::annotations_for([root])).await? {
                    if deletions.is_deleted(&event) {
                        continue;
                    }
                    if event.kind == COVER_NOTE_KIND && seen_cover_notes.insert(event.id) {
                        cover_notes.push(event);
                    } else if event.kind == Kind::Label && seen_labels.insert(event.id) {
                        labels.push(event);
                    }
                }
            }

            sort_newest_first(&mut issues);
            sort_newest_first(&mut patches);
            sort_newest_first(&mut pull_requests);
            sort_oldest_first(&mut comments);
            sort_newest_first(&mut cover_notes);
            sort_newest_first(&mut labels);

            // Resolve every root's status once here; render paths do
            // HashMap lookups instead of scanning all status events per
            // root (quadratic, with an allocation per pair).
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
                cover_notes,
                labels,
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
                cover_notes,
                labels,
            ) = match work.await {
                Ok(data) => data,
                Err(e) => {
                    return this.update(cx, |this, cx| {
                        this.refreshing = false;
                        this.last_error = Some(e.to_string());
                        cx.notify();
                    });
                }
            };

            let again = this.update(cx, |this, cx| {
                this.announcement = announcement;

                // The announcement may list relays for this repository's
                // activity; connect to any we haven't fetched from yet.
                let relays = this
                    .announcement
                    .as_ref()
                    .map(|a| a.relays.clone())
                    .unwrap_or_default();
                this.connect_announced_relays(&relays, cx);

                if let Some((refs, head)) = state {
                    this.refs = refs;
                    this.head = head;
                }

                this.issues = issues;
                this.patches = patches;
                this.pull_requests = pull_requests;
                this.comments = comments;
                this.status_by_root = status_by_root;
                this.open_issue_count = open_issue_count;
                this.open_pr_count = open_pr_count;
                this.cover_notes = cover_notes;
                this.labels = labels;
                this.version = this.version.wrapping_add(1);

                // Comments, statuses without an `a` tag, cover notes and
                // labels are not addressed to the repository, so fetch them
                // by the root events they reference, on the bootstrap relays
                // and on the relays this repository announced.
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
                    // Batch the per-root filters: one statuses filter and one
                    // annotations filter covering all new roots, instead of
                    // one filter per root (each filter is a separate
                    // negentropy reconciliation per relay).
                    let mut root_filters = filters::comments_for(new_roots.clone());
                    root_filters.push(filters::statuses_for(new_roots.iter().copied()));
                    root_filters.push(filters::annotations_for(new_roots));
                    let announced: Vec<RelayUrl> = this.repo_relays.iter().cloned().collect();
                    let backend = Backend::global(cx);
                    backend.update(cx, |backend, cx| {
                        backend.subscribe_bootstrap(root_filters.clone(), cx);
                        backend.connect_repo_relays(announced, root_filters, cx);
                    });
                }

                cx.notify();

                this.refreshing = false;
                if this.refresh_dirty {
                    this.refresh_dirty = false;
                    true
                } else {
                    false
                }
            })?;

            // Requests that arrived while the refresh was running are
            // coalesced into one follow-up refresh.
            if again {
                this.update(cx, |this, cx| this.refresh(cx))?;
            }

            Ok(())
        }));
    }

    /// Resolve the status of a root event (issue / patch / PR) per NIP-34:
    /// a lookup into the map built on the last refresh.
    pub fn status_of(&self, root: &Event) -> RepoStatus {
        status_of(&self.status_by_root, root)
    }

    /// Refresh generation, incremented on every applied refresh. Views use
    /// it to key their derived-data caches (filtered lists, counts) so
    /// renders that change nothing stay O(1).
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The effective cover note of `root` (kind 1624), if any: the latest
    /// note authored by the root author or a maintainer.
    pub fn cover_note_of(&self, root: &Event) -> Option<&Event> {
        let maintainers = self
            .announcement
            .as_ref()
            .map(Announcement::effective_maintainers)
            .unwrap_or_default();

        cover_note(root, &self.cover_notes, &maintainers)
    }

    /// The effective hashtag labels of `root`: its own `t` tags plus labels
    /// from authorized NIP-32 kind-1985 events (`#t` namespace).
    pub fn labels_of(&self, root: &Event) -> Vec<String> {
        let maintainers = self
            .announcement
            .as_ref()
            .map(Announcement::effective_maintainers)
            .unwrap_or_default();

        let (labels, _) = labels_and_subject(root, &self.labels, &maintainers);
        labels
    }

    /// The effective subject/title override of `root` from authorized
    /// kind-1985 events (`#subject` namespace), if any.
    pub fn subject_of(&self, root: &Event) -> Option<String> {
        let maintainers = self
            .announcement
            .as_ref()
            .map(Announcement::effective_maintainers)
            .unwrap_or_default();

        subject_override(root, &self.labels, &maintainers)
    }

    /// Number of open issues: issues whose resolved status is
    /// [`RepoStatus::Open`] (issues without status events default to open).
    /// Cached on the last refresh.
    pub fn issue_count(&self) -> usize {
        self.open_issue_count
    }

    /// Number of open pull requests: root PR events (not PR updates, whose
    /// status is carried by the root) with a resolved status of
    /// [`RepoStatus::Open`]. Cached on the last refresh.
    pub fn pull_request_count(&self) -> usize {
        self.open_pr_count
    }

    /// Whether `user` is the author (owner) of this repository: the public
    /// key of the repository address. Only the author may manage the
    /// repository's pull requests (close / reopen / merge).
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

    /// Comments on a root event (issue / PR), oldest first.
    pub fn comments_of(&self, root: &EventId) -> impl Iterator<Item = &Event> {
        self.comments
            .iter()
            .filter(move |e| signed_core::references_root(e, root))
    }

    /// Comment on a root event (issue / PR) per NIP-34 (kind 1111), using
    /// the SDK's NIP-22 `CommentBuilder` so other NIP-34 clients (ngit,
    /// GitWorkshop) can thread the comment.
    pub fn comment(&mut self, root: &Event, content: String, cx: &mut Context<Self>) {
        self.reply(root, None, content, cx);
    }

    /// Reply to `parent` (a comment on `root`) with a NIP-22 threaded
    /// comment; `None` publishes a top-level comment on the root itself.
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

    /// Open a pull request on this repository: a root PR event (kind 1618)
    /// whose content is the markdown description, plus a root patch event
    /// (kind 1617) carrying the `git format-patch` output, which the PR
    /// references via an `e` tag (NIP-34).
    ///
    /// The patch is published first so the PR can reference its id. The
    /// proposed commit is parsed from the patch's `From <commit>` header;
    /// without one publishing is refused, because the PR's `c` tag must
    /// carry a real commit id for other NIP-34 clients to verify and apply
    /// the proposal. The `clone` tag carries the announced mirror URLs; the
    /// linked patch is the source of truth until the commit is pushed there.
    pub fn open_pull_request(
        &mut self,
        subject: Option<String>,
        description: String,
        patch: String,
        cx: &mut Context<Self>,
    ) {
        self.last_error = None;

        let Some(current_commit) =
            patch_current_commit(&patch).and_then(|hex| hex.parse::<bitcoin_hashes::Sha1>().ok())
        else {
            self.last_error = Some(
                "Patch must be `git format-patch` output with a `From <commit-id>` header".into(),
            );
            cx.notify();
            return;
        };

        let Ok(root_marker) = Tag::parse(["t", "root"]) else {
            return;
        };
        let commit_hex = current_commit.to_string();
        let mut patch_tags = vec![
            Tag::coordinate(self.addr.clone(), None),
            Tag::public_key(self.addr.public_key),
            root_marker,
        ];
        // NIP-34: the `r` EUC tag lets clients subscribe to all patches of
        // this repository; `commit`/`r` tags reference the proposed commit.
        if let Some(euc) = self.announcement.as_ref().and_then(|a| a.euc.clone())
            && let Ok(tag) = Tag::parse(["r", &euc])
        {
            patch_tags.push(tag);
        }
        if let Ok(tag) = Tag::parse(["commit", &commit_hex]) {
            patch_tags.push(tag);
        }
        if let Ok(tag) = Tag::parse(["r", &commit_hex]) {
            patch_tags.push(tag);
        }
        let patch_builder = EventBuilder::new(Kind::GitPatch, patch).tags(patch_tags);

        let patch_task =
            Backend::global(cx).update(cx, |backend, cx| backend.send(patch_builder, cx));

        self.tasks.push(cx.spawn(async move |this, cx| {
            let patch_event = match patch_task.await {
                Ok(event) => event,
                Err(e) => {
                    return this.update(cx, |this, cx| {
                        this.last_error = Some(e.to_string());
                        cx.notify();
                    });
                }
            };

            // The PR references the patch event so viewers can find the
            // patch without carrying it inline.
            let pr_task = this.update(cx, |this, cx| {
                let builder = GitPullRequest {
                    repository: this.addr.clone(),
                    content: description,
                    subject,
                    labels: Vec::new(),
                    branch_name: None,
                    // NIP-34: PRs carry at least one clone URL where the
                    // tip commit can be downloaded; use the repository's
                    // announced mirrors until a push backend exists.
                    clone: this
                        .announcement
                        .as_ref()
                        .map(|a| a.clone.clone())
                        .unwrap_or_default(),
                    current_commit,
                    root_patch_event: Some(patch_event.id),
                    merge_base: None,
                }
                .into_event_builder();

                Backend::global(cx).update(cx, |backend, cx| backend.send(builder, cx))
            })?;

            if let Err(e) = pr_task.await {
                return this.update(cx, |this, cx| {
                    this.last_error = Some(e.to_string());
                    cx.notify();
                });
            }

            Ok(())
        }));
    }

    /// Set the status of a root event. Per NIP-34 only the root author or a
    /// repository maintainer may set the status; status events from anyone
    /// else are ignored by clients, so refuse them up front.
    pub fn set_status(&mut self, root: &Event, status: RepoStatus, cx: &mut Context<Self>) {
        self.last_error = None;

        let maintainers = self
            .announcement
            .as_ref()
            .map(Announcement::effective_maintainers)
            .unwrap_or_default();

        let Some(user) = Backend::global(cx).read(cx).current_user() else {
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

    /// Publish a repository state announcement (kind 30618) with the refs of
    /// the local clone: branches, tags and HEAD. Only the repository owner
    /// may publish state, and a local clone must exist to read the refs from.
    pub fn publish_state(&mut self, cx: &mut Context<Self>) {
        self.last_error = None;

        let Some(user) = Backend::global(cx).read(cx).current_user() else {
            self.last_error = Some("Sign in to publish repository state".into());
            cx.notify();
            return;
        };
        if !self.is_author(&user) {
            self.last_error = Some("Only the repository owner can publish state".into());
            cx.notify();
            return;
        }

        let cache = GitStore::global(cx).cache().clone();
        let addr = self.addr.clone();
        let clone_urls: Vec<String> = self
            .announcement
            .as_ref()
            .map(|a| a.clone.iter().map(ToString::to_string).collect())
            .unwrap_or_default();

        let work = cx.background_spawn(async move {
            let repo = cache.ensure_clone(&addr, &clone_urls)?;
            signed_git::repo_ref_state(&repo)
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            let state = match work.await {
                Ok(state) => state,
                Err(e) => {
                    return this.update(cx, |this, cx| {
                        this.last_error = Some(e.to_string());
                        cx.notify();
                    });
                }
            };

            this.update(cx, |this, cx| {
                let builder =
                    build_state(&this.addr.identifier, &state.refs, state.head.as_deref());
                this.send(builder, cx);
            })?;

            Ok(())
        }));
    }

    /// Merge a pull request: apply its patch (the content of the linked
    /// root patch event) to the local clone of this repository, then publish
    /// the merged status.
    ///
    /// Only the repository author may merge. The clone is created on demand
    /// from the announcement's clone URLs when needed. Patch application
    /// (`git am`) runs on a background thread; failures (e.g. a patch that
    /// no longer applies) surface in [`Self::last_error`].
    pub fn merge_pull_request(&mut self, root: &Event, cx: &mut Context<Self>) {
        self.last_error = None;

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
        let root = root.clone();

        let apply = cx.background_spawn(async move {
            let repo = cache.ensure_clone(&addr, &clone_urls)?;
            let workdir = repo
                .workdir()
                .ok_or_else(|| anyhow::anyhow!("repository has no worktree"))?;
            signed_git::apply_patch(workdir, &patch)
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            match apply.await {
                Ok(()) => {
                    this.update(cx, |this, cx| {
                        this.set_status(&root, RepoStatus::Applied, cx);
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

/// Status of `root` from the precomputed map; roots without status events
/// default to [`RepoStatus::Open`], like [`signed_core::resolve_status`].
fn status_of(status_by_root: &HashMap<EventId, RepoStatus>, root: &Event) -> RepoStatus {
    status_by_root
        .get(&root.id)
        .copied()
        .unwrap_or(RepoStatus::Open)
}

/// Resolve the status of every root event in one pass: status events are
/// indexed by the root they reference (`e`/`E` tag), then each root
/// resolves against its own slice. O(roots + statuses) instead of the
/// O(roots × statuses) of resolving per root on demand.
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
            let status = signed_core::resolve_status(events.iter().copied(), &root.pubkey, maintainers);
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

/// The proposed commit of a `git format-patch` output: the `From <commit>`
/// header on its first line.
fn patch_current_commit(patch: &str) -> Option<&str> {
    let line = patch.lines().next()?;
    let hex = line.strip_prefix("From ")?;
    hex.split_whitespace().next().filter(|hex| hex.len() == 40)
}

/// Build a NIP-22 kind-1111 comment: uppercase `E`/`K`/`P` tags scope the
/// thread root, lowercase `e`/`k`/`p` the direct parent (or the root for a
/// top-level comment). An `a` tag with the repository coordinate (not part
/// of NIP-22) is added so Signed's own activity subscriptions also match.
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

        // The uppercase `E` tag scopes the root: id, relay hint and author.
        let e = event.tags.iter().find(|t| t.kind() == "E").expect("E tag");
        let slice = e.as_slice();
        assert_eq!(slice[1], root.id.to_hex());
        assert_eq!(slice[2], relay.as_str());
        assert_eq!(slice[3], root.pubkey.to_hex());

        // The lowercase `e` tag references the parent, which for a top-level
        // comment is the root itself.
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

        // The uppercase `E` tag still scopes the root event, while the
        // lowercase `e` tag references the parent comment.
        let root_ref = event.tags.iter().find(|t| t.kind() == "E").expect("E tag");
        let parent_ref = event.tags.iter().find(|t| t.kind() == "e").expect("e tag");
        assert_eq!(root_ref.as_slice()[1], root.id.to_hex());
        assert_eq!(parent_ref.as_slice()[1], parent.id.to_hex());

        // The reply still threads under the root for Signed's own display.
        assert!(signed_core::references_root(&event, &root.id));
    }
}
