use anyhow::Error;
use gpui::{Context, Subscription, Task};
use nostr_sdk::prelude::*;
use signed_core::{Announcement, RepoAddr, RepoStatus, filters};

use crate::backend::{Backend, BackendEvent};

/// Per-repository store: announcement, state, issues, patches, PRs and
/// their resolved statuses. Always derived from the local database.
pub struct RepoStore {
    addr: RepoAddr,
    addr_string: String,
    pub announcement: Option<Announcement>,
    /// `(refname, commit-id)` pairs from the latest state announcement.
    pub refs: Vec<(String, String)>,
    /// Branch pointed to by `HEAD` in the latest state announcement.
    pub head: Option<String>,
    pub issues: Vec<Event>,
    pub patches: Vec<Event>,
    pub pull_requests: Vec<Event>,
    statuses: Vec<Event>,
    /// Error of the last action initiated from this store, if any.
    pub last_error: Option<String>,
    refreshing: bool,
    refresh_dirty: bool,
    tasks: Vec<Task<Result<(), Error>>>,
    _subscription: Subscription,
}

impl RepoStore {
    pub fn new(addr: RepoAddr, cx: &mut Context<Self>) -> Self {
        let addr_string = addr.to_string();

        let subscription = cx.subscribe(&Backend::global(cx), |this, _backend, event, cx| {
            let relevant = match event {
                BackendEvent::NostrUpdate(update) => {
                    update.coordinate.as_deref() == Some(this.addr_string.as_str())
                        || (update.kind == Kind::GitRepoAnnouncement
                            && update.author == this.addr.owner)
                }
                BackendEvent::Published(event) => {
                    event.kind == Kind::GitRepoAnnouncement && event.pubkey == this.addr.owner
                        || event.tags.iter().any(|t| {
                            t.kind() == "a" && t.content() == Some(this.addr_string.as_str())
                        })
                }
                _ => false,
            };

            if relevant {
                this.refresh(cx);
            }
        });

        let mut store = Self {
            addr,
            addr_string,
            announcement: None,
            refs: Vec::new(),
            head: None,
            issues: Vec::new(),
            patches: Vec::new(),
            pull_requests: Vec::new(),
            statuses: Vec::new(),
            last_error: None,
            refreshing: false,
            refresh_dirty: false,
            _subscription: subscription,
            tasks: Vec::new(),
        };

        store.subscribe_remote(cx);
        store.refresh(cx);
        store
    }

    pub fn addr(&self) -> &RepoAddr {
        &self.addr
    }

    /// Subscribe the relay pool to this repository's activity.
    fn subscribe_remote(&mut self, cx: &mut Context<Self>) {
        let addr = self.addr.clone();

        Backend::global(cx).update(cx, |backend, cx| {
            backend.subscribe(filters::announcement(&addr), cx);
            backend.subscribe(filters::state(&addr), cx);
            backend.subscribe(filters::activity(&addr), cx);
        });
    }

    /// Re-query the local database and update all fields.
    ///
    /// Debounced: concurrent requests are coalesced into a single re-query
    /// after the running one finishes.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            self.refresh_dirty = true;
            return;
        }
        self.refreshing = true;

        let client = Backend::global(cx).read(cx).client();
        let addr = self.addr.clone();

        let task = cx.spawn(async move |this, cx| {
            loop {
                let queries = async {
                    let db = client.database();

                    let announcements = db.query(filters::announcement(&addr)).await?;
                    let states = db.query(filters::state(&addr)).await?;
                    let activity = db.query(filters::activity(&addr)).await?;

                    Ok::<_, Error>((announcements, states, activity))
                }
                .await;

                let (announcements, states, activity) = match queries {
                    Ok(results) => results,
                    Err(e) => {
                        return this.update(cx, |this, cx| {
                            this.refreshing = false;
                            this.last_error = Some(e.to_string());
                            cx.notify();
                        });
                    }
                };

                let again = this.update(cx, |this, cx| {
                    this.announcement = latest(announcements)
                        .as_ref()
                        .and_then(Announcement::from_event);

                    if let Some(state) = latest(states) {
                        let (refs, head) = parse_state(&state);
                        this.refs = refs;
                        this.head = head;
                    }

                    this.issues.clear();
                    this.patches.clear();
                    this.pull_requests.clear();
                    this.statuses.clear();

                    for event in activity {
                        match event.kind {
                            Kind::GitIssue => this.issues.push(event),
                            Kind::GitPatch => this.patches.push(event),
                            Kind::GitPullRequest | Kind::GitPullRequestUpdate => {
                                this.pull_requests.push(event)
                            }
                            kind if RepoStatus::from_kind(kind).is_some() => {
                                this.statuses.push(event)
                            }
                            _ => {}
                        }
                    }

                    sort_newest_first(&mut this.issues);
                    sort_newest_first(&mut this.patches);
                    sort_newest_first(&mut this.pull_requests);

                    cx.notify();

                    if this.refresh_dirty {
                        this.refresh_dirty = false;
                        true
                    } else {
                        this.refreshing = false;
                        false
                    }
                })?;

                if !again {
                    break;
                }
            }

            Ok(())
        });

        self.tasks.push(task);
    }

    /// Resolve the status of a root event (issue / patch / PR) per NIP-34.
    pub fn status_of(&self, root: &Event) -> RepoStatus {
        let maintainers = self
            .announcement
            .as_ref()
            .map(|a| a.maintainers.as_slice())
            .unwrap_or(&[]);

        let events = self
            .statuses
            .iter()
            .filter(|e| signed_core::references_root(e, &root.id));

        signed_core::resolve_status(events, &root.pubkey, maintainers)
    }

    /// Open an issue on this repository.
    pub fn open_issue(&mut self, subject: Option<String>, content: String, cx: &mut Context<Self>) {
        let builder = GitIssue {
            repository: self.addr.coordinate(),
            content,
            subject,
            labels: Vec::new(),
        }
        .into_event_builder();

        self.send(builder, cx);
    }

    /// Send a root patch (`git format-patch` output) to this repository.
    pub fn send_root_patch(&mut self, patch: String, cx: &mut Context<Self>) {
        let Ok(root_marker) = Tag::parse(["t", "root"]) else {
            return;
        };

        let builder = EventBuilder::new(Kind::GitPatch, patch).tags([
            Tag::coordinate(self.addr.coordinate(), None),
            Tag::public_key(self.addr.owner),
            root_marker,
        ]);

        self.send(builder, cx);
    }

    /// Set the status of a root event (requires being the root author or a maintainer).
    pub fn set_status(&mut self, root: &Event, status: RepoStatus, cx: &mut Context<Self>) {
        let Ok(root_ref) = Tag::parse(["e", &root.id.to_hex(), "", "root"]) else {
            return;
        };

        let builder = EventBuilder::new(status.kind(), "").tags([
            root_ref,
            Tag::public_key(self.addr.owner),
            Tag::public_key(root.pubkey),
            Tag::coordinate(self.addr.coordinate(), None),
        ]);

        self.send(builder, cx);
    }

    fn send(&mut self, builder: EventBuilder, cx: &mut Context<Self>) {
        self.last_error = None;

        let rx = Backend::global(cx).update(cx, |backend, cx| backend.send(builder, cx));

        let task = cx.spawn(async move |this, cx| {
            if let Ok(Err(e)) = rx.recv_async().await {
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

fn latest(events: Events) -> Option<Event> {
    events.into_iter().max_by_key(|e| e.created_at)
}

fn sort_newest_first(events: &mut [Event]) {
    events.sort_by_key(|e| std::cmp::Reverse(e.created_at));
}

/// Parse a kind `30618` state event into refs and HEAD.
fn parse_state(event: &Event) -> (Vec<(String, String)>, Option<String>) {
    let mut refs = Vec::new();
    let mut head = None;

    for tag in event.tags.iter() {
        let kind = tag.kind();
        if kind == "HEAD" {
            head = tag
                .content()
                .and_then(|v| v.strip_prefix("ref: refs/heads/"))
                .map(str::to_owned);
        } else if kind.starts_with("refs/")
            && let Some(commit) = tag.content()
        {
            refs.push((kind.to_owned(), commit.to_owned()));
        }
    }

    (refs, head)
}
