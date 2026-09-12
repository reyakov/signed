use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Error;
use dock::{BasePanel, DockArea, Panel, PanelEvent};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable, ListAlignment, ListState,
    Pixels, Render, SharedString, Stateful, Subscription, Task, WeakEntity, Window, div, list, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{ActiveTheme, Icon, IconName, IconNamed, Sizable, StyledExt, h_flex, v_flex};
use nostr::prelude::{Event, EventId, Kind, PublicKey, Timestamp};
use signed_core::{COVER_NOTE_KIND, InboxItem, InboxReadState, RepoAddr, filters};
use signed_state::{
    Backend, BackendEvent, ProfileStore, RefreshGate, RefreshRequest, RepoListStore, query_inbox,
};
use signed_ui::{CountBadge, UserAvatar};
use utils::relative_time;

use super::{RepoItem, open_repo_item};

/// Delay between a refresh request and the actual re-query.
const REFRESH_DEBOUNCE: Duration = Duration::from_millis(300);

/// Extra list rows measured above and below the visible area.
const LIST_OVERDRAW: Pixels = px(400.);

/// Maximum number of sub-activity lines shown under a thread row.
const MAX_SUB_ACTIVITIES: usize = 5;

/// A repository's slice of the inbox: the threads that belong to it.
struct InboxSection {
    /// Repository the section groups, `None` for items without one.
    address: Option<RepoAddr>,
    /// Number of threads with an unread event.
    unread: usize,
    /// Indices into the threads, newest activity first.
    entries: Vec<usize>,
    /// Timestamp of the newest entry, used to order the sections.
    latest: Timestamp,
}

#[derive(Clone, Copy)]
enum InboxRow {
    Repo(usize),
    Entry(usize, usize),
    Empty,
}

pub struct InboxView {
    focus_handle: FocusHandle,
    dock_area: WeakEntity<DockArea>,
    /// One row per thread, merging notifications and own activity, newest first.
    threads: Arc<Vec<InboxItem>>,
    /// The threads grouped by repository, newest first.
    sections: Arc<Vec<InboxSection>>,
    /// The flattened repository headers and rows of the list.
    rows: Arc<Vec<InboxRow>>,
    /// Number of non-archived threads with an unread event.
    unread_count: usize,
    /// Copy of the global read state the current lists were derived with.
    state: InboxReadState,
    /// Set once the global state has been read for the current user.
    state_loaded: bool,
    refresh: RefreshGate,
    list: ListState,
    tasks: Vec<Task<Result<(), Error>>>,
    _subscriptions: Vec<Subscription>,
}

impl InboxView {
    pub fn new(dock_area: WeakEntity<DockArea>, cx: &mut Context<Self>) -> Self {
        let backend = Backend::global(cx);
        let inbox = backend.read(cx).inbox();
        let repos = RepoListStore::global(cx);
        let weak = cx.entity().downgrade();

        let list = ListState::new(0, ListAlignment::Top, LIST_OVERDRAW);
        let mut subscriptions = vec![];

        subscriptions.push(cx.observe(&inbox, |this, _inbox, cx| {
            this.sync_state(cx);
        }));

        subscriptions.push(cx.subscribe(&backend, |this, _backend, event, cx| {
            this.handle_backend_event(event, cx);
        }));

        // Rebuild when the user's own repositories load or change,
        // so a repository without any activity still gets an empty section.
        subscriptions.push(cx.observe(&repos, |this, _repos, cx| {
            this.rebuild(cx);
            cx.notify();
        }));

        // Derive the sections once the panel exists.
        cx.defer(move |cx| {
            if let Err(error) = weak.update(cx, |this, cx| this.sync_state(cx)) {
                log::warn!("inbox dropped before bootstrap could run: {error}");
            }
        });

        Self {
            focus_handle: cx.focus_handle(),
            dock_area,
            threads: Arc::new(Vec::new()),
            sections: Arc::new(Vec::new()),
            rows: Arc::new(Vec::new()),
            unread_count: 0,
            state: InboxReadState::default(),
            state_loaded: false,
            refresh: RefreshGate::default(),
            list,
            tasks: vec![],
            _subscriptions: subscriptions,
        }
    }

    /// Mark every known notification read.
    pub fn mark_all_read(&mut self, cx: &mut Context<Self>) {
        let Some(me) = Backend::global(cx).read(cx).current_user() else {
            return;
        };

        let all: Vec<Event> = self
            .threads
            .iter()
            .flat_map(|item| item.events.iter().cloned())
            .collect();

        let backend = Backend::global(cx);
        let inbox = backend.read(cx).inbox();

        inbox.update(cx, |inbox, cx| inbox.mark_all_read(&all, me, cx));
    }

    /// Re-derive from the global state when it is loaded or changes.
    pub fn sync_state(&mut self, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let inbox = backend.read(cx).inbox();

        let (loaded, state) = {
            let inbox = inbox.read(cx);
            (inbox.is_loaded(), inbox.state().clone())
        };

        if !loaded {
            let was_present =
                self.state_loaded || !self.threads.is_empty() || !self.sections.is_empty();
            self.clear();
            if was_present {
                cx.notify();
            }
            return;
        }

        if !self.state_loaded {
            self.state_loaded = true;
            self.state = state;
            self.refresh_initial(cx);
            return;
        }

        if self.state != state {
            self.state = state;
            self.regroup(cx);
            cx.notify();
        }
    }

    /// Handle a backend event that can change the derived sections.
    fn handle_backend_event(&mut self, event: &BackendEvent, cx: &mut Context<Self>) {
        match event {
            BackendEvent::NostrUpdate(updates) => {
                let relevant = updates.iter().any(|update| {
                    let is_notification = filters::NOTIFICATION_KINDS.contains(&update.kind);
                    let is_comment = update.kind == Kind::Comment;
                    let is_event_deletion = update.kind == Kind::EventDeletion;
                    let is_request_to_vanish = update.kind == Kind::RequestToVanish;

                    is_notification || is_comment || is_event_deletion || is_request_to_vanish
                });

                if relevant {
                    self.refresh(cx);
                }
            }
            BackendEvent::Synced | BackendEvent::Published(_) => self.refresh(cx),
            _ => {}
        }
    }

    /// One-shot initial load, no debounce.
    fn refresh_initial(&mut self, cx: &mut Context<Self>) {
        debug_assert!(!self.refresh.debouncing());
        if self.refresh.running() {
            self.refresh.request();
            return;
        }
        self.run_refresh(cx);
    }

    /// Re-query the local database.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.state_loaded {
            return;
        }

        if self.refresh.request() != RefreshRequest::Schedule {
            return;
        }

        self.tasks.push(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH_DEBOUNCE).await;
            this.update(cx, |this, cx| this.run_refresh(cx))
        }));
    }

    /// One query and apply cycle, the debounced entry point.
    fn run_refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh.begin();

        let backend = Backend::global(cx);
        let Some(me) = backend.read(cx).current_user() else {
            self.refresh.abort();
            return;
        };

        let client = backend.read(cx).client();
        let state = self.state.clone();

        let work = cx.background_spawn(async move { query_inbox(&client, me, &state).await });

        self.tasks.push(cx.spawn(async move |this, cx| {
            let (threads, unread_count) = match work.await {
                Ok(results) => results,
                Err(error) => {
                    log::warn!("inbox refresh failed: {error}");
                    return this.update(cx, |this, _cx| this.refresh.abort());
                }
            };

            let again = this.update(cx, |this, cx| {
                if backend.read(cx).current_user() != Some(me) {
                    this.refresh.abort();
                    return false;
                }

                this.threads = Arc::new(threads);
                this.unread_count = unread_count;
                this.rebuild(cx);
                cx.notify();

                this.refresh.finish()
            })?;

            if again {
                this.update(cx, |this, cx| this.refresh(cx))?;
            }

            Ok(())
        }));
    }

    /// Recompute the unread and archived flags from the current state.
    fn regroup(&mut self, cx: &mut Context<Self>) {
        let mut items = (*self.threads).clone();

        for item in items.iter_mut() {
            item.apply_state(&self.state);
        }

        self.unread_count = items.iter().filter(|item| item.is_unread()).count();
        self.threads = Arc::new(items);
        self.rebuild(cx);
    }

    /// Regroup the current threads by repository and flatten them into rows.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let backend = Backend::global(cx);
        let repo_list = RepoListStore::global(cx);
        let mut sections = self.group_sections();

        if let Some(me) = backend.read(cx).current_user() {
            for announcement in repo_list.read(cx).announcements_of(&me) {
                let address = announcement.addr();
                let known = sections
                    .iter()
                    .any(|section| section.address.as_ref() == Some(&address));

                if !known {
                    sections.push(InboxSection {
                        address: Some(address),
                        unread: 0,
                        entries: Vec::new(),
                        latest: Timestamp::default(),
                    });
                }
            }
        }

        sections.sort_by_key(|section| std::cmp::Reverse(section.latest));

        let rows = self.flatten_rows(&sections);
        self.sections = Arc::new(sections);
        self.rows = Arc::new(rows);
    }

    /// Group the threads into one section per repository.
    fn group_sections(&self) -> Vec<InboxSection> {
        let mut by_repo: HashMap<Option<RepoAddr>, InboxSection> = HashMap::new();

        for (ix, item) in self.threads.iter().enumerate() {
            if item.archived {
                continue;
            }

            let address = item.address.clone();
            let section = by_repo
                .entry(address.clone())
                .or_insert_with(move || InboxSection {
                    address,
                    unread: 0,
                    entries: Vec::new(),
                    latest: Timestamp::default(),
                });

            if item.is_unread() {
                section.unread += 1;
            }

            section.latest = section.latest.max(item.latest_activity());
            section.entries.push(ix);
        }

        let mut sections: Vec<InboxSection> = by_repo.into_values().collect();

        for section in &mut sections {
            section.entries.sort_by(|a, b| {
                self.threads[*b]
                    .latest_activity()
                    .cmp(&self.threads[*a].latest_activity())
            });
        }

        sections.sort_by_key(|section| std::cmp::Reverse(section.latest));
        sections
    }

    /// Flatten the sections into the list of repository headers and their rows.
    fn flatten_rows(&self, sections: &[InboxSection]) -> Vec<InboxRow> {
        let mut rows = Vec::new();

        for (section_ix, section) in sections.iter().enumerate() {
            rows.push(InboxRow::Repo(section_ix));

            if section.entries.is_empty() {
                rows.push(InboxRow::Empty);
                continue;
            }

            rows.extend(
                (0..section.entries.len()).map(|entry_ix| InboxRow::Entry(section_ix, entry_ix)),
            );
        }

        rows
    }

    /// Forget everything derived for the current user.
    fn clear(&mut self) {
        self.threads = Arc::new(Vec::new());
        self.sections = Arc::new(Vec::new());
        self.rows = Arc::new(Vec::new());
        self.unread_count = 0;
        self.state = InboxReadState::default();
        self.state_loaded = false;
        // Drop any in-flight or pending run belonging to the previous user.
        self.refresh = RefreshGate::default();
    }

    fn open(
        &self,
        root: EventId,
        kind: Option<Kind>,
        address: Option<RepoAddr>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(address) = address else {
            return;
        };

        let Some(announcement) = RepoListStore::global(cx)
            .read(cx)
            .announcements
            .iter()
            .find(|announcement| announcement.addr() == address)
            .cloned()
        else {
            return;
        };

        let item = match kind {
            Some(Kind::GitIssue) => RepoItem::Issue(root),
            Some(Kind::GitPullRequest) => RepoItem::PullRequest(root),
            Some(Kind::GitPatch) => RepoItem::Patch,
            _ => return,
        };

        open_repo_item(&self.dock_area, &announcement, item, window, cx);
    }

    fn render_entry(&self, ix: usize, cx: &Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix) else {
            return div().into_any_element();
        };

        match *row {
            InboxRow::Empty => empty_section_row(cx),
            InboxRow::Repo(section_ix) => {
                let Some(section) = self.sections.get(section_ix) else {
                    return div().into_any_element();
                };
                repo_header(section, cx)
            }
            InboxRow::Entry(section_ix, entry_ix) => {
                let Some(section) = self.sections.get(section_ix) else {
                    return div().into_any_element();
                };

                let Some(&thread_ix) = section.entries.get(entry_ix) else {
                    return div().into_any_element();
                };

                let Some(item) = self.threads.get(thread_ix) else {
                    return div().into_any_element();
                };

                let root = item.root;
                let kind = item.root_kind;
                let address = section.address.clone();
                let first = entry_ix == 0;
                let last = entry_ix + 1 == section.entries.len();

                thread("inbox-row", ix, item, first, last, cx)
                    .on_click(cx.listener(move |this, _ev, window, cx| {
                        this.open(root, kind, address.clone(), window, cx)
                    }))
                    .into_any_element()
            }
        }
    }
}

/// Display name of the repository at `addr`, from the announcement store.
fn repo_name(addr: Option<&RepoAddr>, cx: &App) -> Option<SharedString> {
    let repo_list = RepoListStore::global(cx);
    let addr = addr?;
    repo_list
        .read(cx)
        .announcements
        .iter()
        .find(|announcement| announcement.addr() == *addr)
        .map(|announcement| announcement.name().map(SharedString::from))
}

/// Header of a repository section.
fn repo_header(section: &InboxSection, cx: &App) -> AnyElement {
    let name =
        repo_name(section.address.as_ref(), cx).unwrap_or_else(|| SharedString::from("Untitled"));

    h_flex()
        .h_12()
        .w_full()
        .gap_1()
        .items_center()
        .child(
            div()
                .min_w_0()
                .text_sm()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(name),
        )
        .when(section.unread > 0, |this| {
            this.child(CountBadge::new(section.unread))
        })
        .into_any_element()
}

/// Placeholder under a repository header that has nothing to show.
fn empty_section_row(cx: &App) -> AnyElement {
    h_flex()
        .h_12()
        .w_full()
        .px_3()
        .text_xs()
        .text_color(cx.theme().secondary_foreground)
        .bg(cx.theme().secondary.alpha(0.6))
        .rounded(cx.theme().radius)
        .child(SharedString::from("No activity yet."))
        .into_any_element()
}

fn thread(
    prefix: &'static str,
    ix: usize,
    item: &InboxItem,
    first: bool,
    last: bool,
    cx: &App,
) -> Stateful<Div> {
    let title = SharedString::from(item.title());
    let unread = item.is_unread();

    let backend = Backend::global(cx);
    let me = backend.read(cx).current_user();

    let mut timeline = v_flex().gap_2().w_full();

    for event in item.timeline(MAX_SUB_ACTIVITIES) {
        timeline = timeline.child(sub_activity(&event, me, cx));
    }

    v_flex()
        .id((prefix, ix))
        .w_full()
        .px_3()
        .py_2()
        .gap_2()
        .bg(cx.theme().secondary.alpha(0.6))
        .when(first, |this| this.rounded_t(cx.theme().radius))
        .when(last, |this| this.rounded_b(cx.theme().radius))
        .when(!last, |this| {
            this.border_b_1().border_color(cx.theme().background)
        })
        .hover(|this| this.bg(cx.theme().secondary_hover.alpha(0.8)))
        .child(
            h_flex()
                .gap_2()
                .text_sm()
                .child(
                    h_flex()
                        .size_6()
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .child(Icon::new(IconName::Bell)),
                )
                .child(
                    div()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(title),
                )
                .child(div().flex_1())
                .when(unread, |this| {
                    this.child(
                        div()
                            .flex_shrink_0()
                            .size_2()
                            .rounded_full()
                            .bg(cx.theme().primary),
                    )
                }),
        )
        .child(timeline)
}

fn sub_activity(event: &Event, me: Option<PublicKey>, cx: &App) -> AnyElement {
    let profile_store = ProfileStore::global(cx).read(cx);
    let profile = profile_store.get(&event.pubkey);

    let name = if Some(event.pubkey) == me {
        SharedString::from("You")
    } else {
        profile.name()
    };

    h_flex()
        .w_full()
        .gap_2()
        .items_center()
        .child(div().w_6().flex_shrink_0())
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .items_center()
                .text_xs()
                .child(
                    UserAvatar::new(name.clone())
                        .picture(profile.picture())
                        .xsmall(),
                )
                .child(name)
                .child(SharedString::from(activity_phrase(event.kind)))
                .child(div().flex_1())
                .child(
                    div()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(relative_time(event.created_at))),
                ),
        )
        .into_any_element()
}

/// Phrase describing an activity event, read as `[name] [phrase]`.
fn activity_phrase(kind: Kind) -> &'static str {
    if kind == COVER_NOTE_KIND {
        return "added a note";
    }

    match kind {
        Kind::GitIssue => "opened an issue",
        Kind::GitPullRequest => "opened a PR",
        Kind::GitPullRequestUpdate => "updated a PR",
        Kind::GitPatch => "created a patch",
        Kind::Comment => "commented",
        Kind::GitStatusOpen => "opened a status",
        Kind::GitStatusApplied => "applied a status",
        Kind::GitStatusClosed => "closed a status",
        Kind::GitStatusDraft => "drafted a status",
        _ => "did something",
    }
}

/// Centered muted icon and message filling its container.
fn empty_state(icon: impl IconNamed, message: &str, cx: &App) -> AnyElement {
    v_flex()
        .w_full()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .gap_2()
        .py_8()
        .child(
            Icon::new(icon)
                .large()
                .text_color(cx.theme().muted_foreground),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(message)),
        )
        .into_any_element()
}

impl BasePanel for InboxView {
    fn panel_name(&self) -> &'static str {
        "inbox"
    }
}

impl Panel for InboxView {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().text_sm().child(SharedString::from("Inbox"))
    }
}

impl EventEmitter<PanelEvent> for InboxView {}

impl Focusable for InboxView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for InboxView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows.clone();

        if self.list.item_count() != rows.len() {
            self.list.reset(rows.len());
        }

        v_flex()
            .image_cache(gpui::retain_all("inbox"))
            .size_full()
            .gap_2()
            .child(
                h_flex()
                    .px_4()
                    .h_12()
                    .w_full()
                    .gap_1()
                    .items_center()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child(SharedString::from("Inbox")),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("mark-all")
                            .icon(IconName::CircleCheck)
                            .secondary()
                            .tooltip("Mark all as read")
                            .on_click(cx.listener(move |this, _ev, _window, cx| {
                                this.mark_all_read(cx);
                            })),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .px_4()
                    .when_else(
                        rows.is_empty(),
                        |this| {
                            this.child(empty_state(IconName::Inbox, "You're all caught up.", cx))
                        },
                        |this| {
                            this.child(
                                list(
                                    self.list.clone(),
                                    cx.processor(|this, ix, _window, cx| this.render_entry(ix, cx)),
                                )
                                .size_full()
                                .min_h_0()
                                .into_any_element(),
                            )
                        },
                    )
                    .child(div().h_6().w_full().flex_shrink_0()),
            )
    }
}
