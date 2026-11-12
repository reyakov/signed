use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Error;
use assets::CustomIconName;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, WeakEntity,
    Window, div,
};
use gpui_native_kit::button::Button;
use gpui_native_kit::common::{h_flex, v_flex};
use gpui_native_kit::icons::{Icon, IconRole};
use gpui_native_kit::layout::dock::{DockState, Panel};
use gpui_native_kit::scroll_area::ScrollArea;
use gpui_native_kit::theme::{ActiveTheme as _, TextRole, Typeset as _};
use metadata::ProfileDirectory;
use nostr::prelude::{Event, EventId, Kind, PublicKey, Timestamp};
use signed_core::{InboxItem, InboxReadState, RepoAddr};
use signed_state::{Backend, BackendEvent, Inbox, RefreshGate, RefreshRequest, RepoListStore};

mod helpers;

use helpers::{empty_section_row, empty_state, repo_header, thread};

use crate::repo::actions::{RepoItem, open_repo_item};

const MAX_SUB_ACTIVITIES: usize = 5;

#[derive(Clone, Copy)]
enum InboxRow {
    Repo(usize),
    Entry(usize, usize),
    Empty,
}

struct InboxSection {
    address: Option<RepoAddr>,
    unread: usize,
    entries: Vec<usize>,
    latest: Timestamp,
}

pub struct InboxView {
    dock: WeakEntity<DockState>,
    threads: Arc<Vec<InboxItem>>,
    sections: Arc<Vec<InboxSection>>,
    rows: Arc<Vec<InboxRow>>,
    state: InboxReadState,
    state_loaded: bool,
    refresh: RefreshGate,
    tasks: Vec<Task<Result<(), Error>>>,
    directory: Entity<ProfileDirectory>,
    _subscriptions: Vec<Subscription>,
}

impl InboxView {
    pub fn new(dock: WeakEntity<DockState>, cx: &mut Context<Self>) -> Self {
        let backend = Backend::global(cx);
        let inbox = backend.read(cx).inbox();

        let repos = RepoListStore::global(cx);
        let weak = cx.entity().downgrade();

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
            dock,
            threads: Arc::new(Vec::new()),
            sections: Arc::new(Vec::new()),
            rows: Arc::new(Vec::new()),
            state: InboxReadState::default(),
            state_loaded: false,
            refresh: RefreshGate::default(),
            tasks: vec![],
            directory: ProfileDirectory::new(cx),
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
                    return this.update(cx, |this, _cx| this.refresh.abort());
                }
            };

            let again = this.update(cx, |this, cx| {
                if backend.read(cx).current_user() != Some(me) {
                    this.refresh.abort();
                    return false;
                }
                this.threads = Arc::new(threads);
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
            rows.extend(
                (0..section.entries.len()).map(|entry_ix| InboxRow::Entry(section_ix, entry_ix)),
            );
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
        open_repo_item(&self.dock, &address, None, item, window, cx);
    }

    fn render_entry(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix) else {
            return div().into_any_element();
        };

        match *row {
            InboxRow::Empty => empty_section_row(cx),
            InboxRow::Repo(section_ix) => {
                let Some(section) = self.sections.get(section_ix) else {
                    return div().into_any_element();
                };
                let name = repo_name(section.address.as_ref(), cx);
                let id = SharedString::from(format!(
                    "inbox-unread:{}",
                    section
                        .address
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default()
                ));
                repo_header(id, name, section.unread, cx)
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
                let kind = item.root_event.as_ref().map(|event| event.kind);
                let address = section.address.clone();
                let first = entry_ix == 0;
                let last = entry_ix + 1 == section.entries.len();
                let me = Backend::global(cx).read(cx).current_user();

                thread(
                    "inbox-row",
                    ix,
                    SharedString::from(item.title()),
                    item.is_unread(),
                    first,
                    last,
                    item.timeline(MAX_SUB_ACTIVITIES),
                    me,
                    &self.directory,
                    cx,
                )
                .on_click(cx.listener(move |this, _ev, window, cx| {
                    this.open(root, kind, address.clone(), window, cx)
                }))
                .into_any_element()
            }
        }
    }
}

impl Panel for InboxView {
    fn title(&self, _cx: &App) -> Option<SharedString> {
        Some(SharedString::from("Inbox"))
    }

    fn lead(&self, cx: &App) -> Option<AnyElement> {
        Some(
            Icon::named(CustomIconName::Inbox)
                .size(gpui::px(14.))
                .color(cx.theme().colors.muted_foreground)
                .into_any_element(),
        )
    }
}

impl Render for InboxView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows.clone();
        let mut children = Vec::with_capacity(rows.len());
        for ix in 0..rows.len() {
            children.push(self.render_entry(ix, cx));
        }

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
                            .typeset(TextRole::Headline, cx)
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
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .px_4()
                    .child(if rows.is_empty() {
                        empty_state("You're all caught up.", cx)
                    } else {
                        ScrollArea::new("inbox-scroll")
                            .flex_1()
                            .child(v_flex().gap_2().py_2().children(children))
                            .into_any_element()
                    }),
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
        .map(|announcement| {
            announcement
                .name
                .as_deref()
                .map(SharedString::from)
                .unwrap_or_else(|| SharedString::from(announcement.id.clone()))
        })
        .unwrap_or_else(|| SharedString::from("Untitled"))
}
