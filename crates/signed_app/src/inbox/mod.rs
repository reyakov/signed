use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::Error;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ListAlignment, ListState,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, WeakEntity, Window, div, list, px,
};
use metadata::ProfileDirectory;
use nostr::prelude::{Event, EventId, Kind, PublicKey, Timestamp};
use signed_core::{InboxItem, InboxReadState, RepoAddr};
use signed_state::{Backend, BackendEvent, Inbox, RefreshGate, RefreshRequest, RepoListStore};
use ui::alert::{Alert, AlertSeverity, AlertStyle};
use ui::button::Button;
use ui::common::{StyledExt as _, h_flex, v_flex};
use ui::empty_state::EmptyState;
use ui::icons::{Icon, IconRole};
use ui::layout::panel::Panel;

mod helpers;

use crate::app::{AppRoot, RepoItem};
use crate::shared::labels::AnnouncementExt;
use crate::shared::log_dropped;

const MAX_SUB_ACTIVITIES: usize = 5;

/// Opens a thread's root event in its repository.
type OpenHandler = Rc<dyn Fn(EventId, Option<Kind>, Option<RepoAddr>, &mut Window, &mut App)>;

#[derive(Clone, Copy)]
enum InboxRow {
    Repo(usize),
    Entry(usize),
    Empty,
}

/// One thread row's display data, built when the inbox regroups.
struct EntryRow {
    root: EventId,
    kind: Option<Kind>,
    address: Option<RepoAddr>,
    title: SharedString,
    unread: bool,
    first: bool,
    last: bool,
    timeline: Arc<[Event]>,
}

struct InboxSection {
    address: Option<RepoAddr>,
    /// The section's display name.
    name: SharedString,
    /// The unread badge's element id.
    badge_id: SharedString,
    unread: usize,
    entries: Vec<usize>,
    latest: Timestamp,
}

pub struct InboxView {
    workspace: WeakEntity<AppRoot>,
    threads: Arc<Vec<InboxItem>>,
    sections: Arc<Vec<InboxSection>>,
    rows: Arc<Vec<InboxRow>>,
    /// The thread rows, indexed by `InboxRow::Entry`.
    entry_rows: Arc<Vec<EntryRow>>,
    /// Scroll state of the flattened rows.
    rows_scroll: ListState,
    state: InboxReadState,
    state_loaded: bool,
    refresh: RefreshGate,
    tasks: Vec<Task<Result<(), Error>>>,
    directory: Entity<ProfileDirectory>,
    /// The last refresh failure, shown inline until the next refresh succeeds.
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl InboxView {
    pub fn new(workspace: WeakEntity<AppRoot>, cx: &mut Context<Self>) -> Self {
        let backend = Backend::global(cx);
        let inbox = backend.read(cx).inbox();

        let repos = RepoListStore::global(cx);
        let weak = cx.weak_entity();

        let mut subscriptions = vec![];

        subscriptions.push(cx.observe(&inbox, |this, _inbox, cx| {
            this.sync_state(cx);
        }));

        subscriptions.push(cx.subscribe(&backend, |this, _backend, event, cx| {
            this.handle_backend_event(event, cx);
        }));

        subscriptions.push(cx.observe(&repos, |this, _repos, cx| {
            this.rebuild(cx);
            cx.notify();
        }));

        cx.defer(move |cx| {
            if let Err(error) = weak.update(cx, |this, cx| this.sync_state(cx)) {
                log::warn!("inbox dropped before bootstrap could run: {error}");
            }
        });

        Self {
            workspace,
            threads: Arc::new(Vec::new()),
            sections: Arc::new(Vec::new()),
            rows: Arc::new(Vec::new()),
            entry_rows: Arc::new(Vec::new()),
            rows_scroll: ListState::new(0, ListAlignment::Top, px(48.)),
            state: InboxReadState::default(),
            state_loaded: false,
            refresh: RefreshGate::default(),
            tasks: vec![],
            directory: ProfileDirectory::new(cx),
            error: None,
            _subscriptions: subscriptions,
        }
    }

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

    fn handle_backend_event(&mut self, event: &BackendEvent, cx: &mut Context<Self>) {
        match event {
            BackendEvent::RepoUpdates(_) | BackendEvent::Synced => self.refresh(cx),
            _ => {}
        }
    }

    fn refresh_initial(&mut self, cx: &mut Context<Self>) {
        if self.refresh.running() {
            self.refresh.request();
            return;
        }
        self.run_refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.state_loaded || self.refresh.request() != RefreshRequest::Schedule {
            return;
        }
        self.run_refresh(cx);
    }

    fn run_refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh.begin();
        let backend = Backend::global(cx);
        let (me, client) = {
            let backend = backend.read(cx);
            (backend.current_user(), backend.client())
        };
        let Some(me) = me else {
            self.refresh.abort();
            return;
        };
        let state = self.state.clone();
        let work = cx.background_spawn(async move { Inbox::query(&client, me, &state).await });

        self.tasks.push(cx.spawn(async move |this, cx| {
            let (threads, _unread_count) = match work.await {
                Ok(results) => results,
                Err(error) => {
                    log::warn!("inbox refresh failed: {error}");
                    return this.update(cx, |this, cx| {
                        this.error = Some(error.to_string().into());
                        this.refresh.abort();
                        cx.notify();
                    });
                }
            };

            let again = this.update(cx, |this, cx| {
                if backend.read(cx).current_user() != Some(me) {
                    this.refresh.abort();
                    return false;
                }
                this.threads = Arc::new(threads);
                this.error = None;
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

    fn regroup(&mut self, cx: &mut Context<Self>) {
        let mut items = (*self.threads).clone();
        for item in items.iter_mut() {
            item.apply_state(&self.state);
        }
        self.threads = Arc::new(items);
        self.rebuild(cx);
    }

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
                        name: SharedString::default(),
                        badge_id: SharedString::default(),
                        unread: 0,
                        entries: Vec::new(),
                        latest: Timestamp::default(),
                    });
                }
            }
        }

        sections.sort_by_key(|section| std::cmp::Reverse(section.latest));

        let mut entry_rows = Vec::new();
        for section in &mut sections {
            let name = repo_name(section.address.as_ref(), cx);
            section.badge_id = SharedString::from(match &section.address {
                Some(address) => format!("inbox-unread:{address}"),
                None => String::from("inbox-unread:local"),
            });
            section.name = name;

            let last = section.entries.len().saturating_sub(1);
            for (entry_ix, thread_ix) in section.entries.iter().enumerate() {
                let Some(item) = self.threads.get(*thread_ix) else {
                    continue;
                };
                entry_rows.push(EntryRow {
                    root: item.root,
                    kind: item.root_event.as_ref().map(|event| event.kind),
                    address: section.address.clone(),
                    title: SharedString::from(item.title()),
                    unread: item.is_unread(),
                    first: entry_ix == 0,
                    last: entry_ix == last,
                    timeline: item.timeline(MAX_SUB_ACTIVITIES).into(),
                });
            }
        }

        let rows = self.flatten_rows(&sections);
        self.sections = Arc::new(sections);
        self.entry_rows = Arc::new(entry_rows);
        self.rows = Arc::new(rows);

        let mut keys: Vec<PublicKey> = self
            .threads
            .iter()
            .flat_map(|item| item.events.iter().map(|event| event.pubkey))
            .collect();
        keys.sort_by_key(PublicKey::to_hex);
        keys.dedup();
        self.directory
            .update(cx, |directory, cx| directory.track(keys, cx));
    }

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
                    name: SharedString::default(),
                    badge_id: SharedString::default(),
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

    fn flatten_rows(&self, sections: &[InboxSection]) -> Vec<InboxRow> {
        let mut rows = Vec::new();
        for (section_ix, section) in sections.iter().enumerate() {
            rows.push(InboxRow::Repo(section_ix));
            if section.entries.is_empty() {
                rows.push(InboxRow::Empty);
                continue;
            }
            rows.extend((0..section.entries.len()).map(InboxRow::Entry));
        }
        rows
    }

    fn clear(&mut self) {
        self.threads = Arc::new(Vec::new());
        self.sections = Arc::new(Vec::new());
        self.rows = Arc::new(Vec::new());
        self.state = InboxReadState::default();
        self.state_loaded = false;
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
        let item = match kind {
            Some(Kind::GitIssue) => RepoItem::Issue(root),
            Some(Kind::GitPullRequest) => RepoItem::PullRequest(root),
            Some(Kind::GitPatch) => RepoItem::Patch,
            _ => return,
        };
        let result = self.workspace.update(cx, |root, cx| {
            root.open_repo_item(&address, None, item, window, cx);
        });
        log_dropped(result);
    }

    fn render_entry(&self, ix: usize, on_open: &OpenHandler, cx: &App) -> AnyElement {
        let Some(row) = self.rows.get(ix) else {
            return div().into_any_element();
        };

        match *row {
            InboxRow::Empty => InboxSection::empty_row(cx).into_any_element(),
            InboxRow::Repo(section_ix) => {
                let Some(section) = self.sections.get(section_ix) else {
                    return div().into_any_element();
                };
                section.header(cx).into_any_element()
            }
            InboxRow::Entry(entry_ix) => {
                let Some(row) = self.entry_rows.get(entry_ix) else {
                    return div().into_any_element();
                };
                let me = Backend::global(cx).read(cx).current_user();

                self.thread(row, me, cx)
                    .on_click({
                        let on_open = on_open.clone();
                        let root = row.root;
                        let kind = row.kind;
                        let address = row.address.clone();
                        move |_, window, cx| on_open(root, kind, address.clone(), window, cx)
                    })
                    .into_any_element()
            }
        }
    }
}

impl Panel for InboxView {
    fn title(&self, _cx: &App) -> SharedString {
        SharedString::from("Inbox")
    }
}

impl Render for InboxView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.rows.len();
        if self.rows_scroll.item_count() != count {
            self.rows_scroll.reset(count);
        }
        let weak = cx.weak_entity();

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_4()
                    .h_12()
                    .w_full()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .child(SharedString::from("Inbox")),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("mark-all")
                            .icon(Icon::named(IconRole::Check))
                            .label("Mark all read")
                            .on_click(cx.listener(|this, _, _, cx| this.mark_all_read(cx))),
                    ),
            )
            .when_some(self.error.clone(), |this, error| {
                let weak = cx.weak_entity();
                this.child(
                    Alert::new("inbox-error")
                        .severity(AlertSeverity::Danger)
                        .title("Inbox")
                        .message(error)
                        .style(AlertStyle::Inline)
                        .on_dismiss(move |cx| {
                            if let Err(error) = weak.update(cx, |this, cx| {
                                this.error = None;
                                cx.notify();
                            }) {
                                log::warn!("failed to dismiss inbox error: {error}");
                            }
                        }),
                )
            })
            .child(
                div().relative().flex_1().min_h_0().px_4().child(
                    v_flex()
                        .flex_1()
                        .py_2()
                        .when(count == 0, |this| {
                            this.child(EmptyState::centered("You're all caught up."))
                        })
                        .when(count > 0, |this| {
                            this.child(list(self.rows_scroll.clone(), {
                                let weak = weak.clone();
                                let open = cx.processor(
                                    |this,
                                     (root, kind, address): (
                                        EventId,
                                        Option<Kind>,
                                        Option<RepoAddr>,
                                    ),
                                     window,
                                     cx| {
                                        this.open(root, kind, address, window, cx);
                                    },
                                );
                                let on_open: OpenHandler =
                                    Rc::new(move |root, kind, address, window, cx| {
                                        open((root, kind, address), window, cx);
                                    });
                                move |ix, _window, cx| {
                                    weak.read_with(cx, |view, cx| {
                                        view.render_entry(ix, &on_open, cx)
                                    })
                                    .unwrap_or_else(|_| div().into_any_element())
                                }
                            }))
                        }),
                ),
            )
            .into_any_element()
    }
}

fn repo_name(addr: Option<&RepoAddr>, cx: &App) -> SharedString {
    let Some(addr) = addr else {
        return SharedString::from("Untitled");
    };
    RepoListStore::global(cx)
        .read(cx)
        .announcements
        .iter()
        .find(|announcement| announcement.addr() == *addr)
        .map(AnnouncementExt::display_name)
        .unwrap_or_else(|| SharedString::from("Untitled"))
}
