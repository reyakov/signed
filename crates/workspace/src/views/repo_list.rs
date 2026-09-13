use std::fmt::Display;
use std::rc::Rc;

use assets::CustomIconName;
use dock::{BasePanel, DockArea, Panel, PanelEvent};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels, Render,
    SharedString, Size, Subscription, WeakEntity, Window, div, px, relative, size,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::Scrollbar;
use gpui_component::{
    ActiveTheme, Icon, IconName, Sizable, StyledExt, VirtualListScrollHandle, h_flex, v_flex,
    v_virtual_list,
};
use signed_core::Announcement;
use signed_state::{ProfileStore, RepoListStore, Timestamp};
use signed_ui::{SegmentButton, UserAvatar};
use utils::relative_time;

use super::open_repo_panel;

const COLUMNS: usize = 2;
const CARD_HEIGHT: f32 = 40. + 64. + 48. + 2. + 6.;

const RECENT_COUNT: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum RepoFilter {
    All,
    #[default]
    Popular,
    Recent,
}

impl AsRef<str> for RepoFilter {
    fn as_ref(&self) -> &str {
        match self {
            RepoFilter::All => "all",
            RepoFilter::Popular => "popular",
            RepoFilter::Recent => "recent",
        }
    }
}

impl Display for RepoFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_ref())
    }
}

impl RepoFilter {
    /// Indices into the store's `announcements` this filter includes, in display order.
    ///
    /// Narrowed to repositories whose name or id contains `query`.
    fn visible(self, store: &RepoListStore, query: &str) -> Vec<usize> {
        let announcements = &store.announcements;
        let mut indices: Vec<usize> = (0..announcements.len()).collect();

        // Narrow by the search query first.
        // Recent then limits the matches and Popular ranks them.
        let query = query.trim().to_lowercase();

        if !query.is_empty() {
            indices.retain(|&ix| {
                let announcement = &announcements[ix];
                let name = announcement.name.as_deref().unwrap_or(&announcement.id);
                name.to_lowercase().contains(&query)
            });
        }

        match self {
            Self::All => {}
            Self::Recent => indices.truncate(RECENT_COUNT),
            Self::Popular => {
                let counts = &store.counts;
                let scores: Vec<u32> = announcements
                    .iter()
                    .map(|a| counts.get(&a.addr()).map_or(0, |c| c.score()))
                    .collect();
                indices.sort_by(|&a, &b| scores[b].cmp(&scores[a]));
            }
        }

        indices
    }

    fn icon_name(self) -> CustomIconName {
        match self {
            Self::All => CustomIconName::Grid,
            Self::Recent => CustomIconName::Recent,
            Self::Popular => CustomIconName::Trending,
        }
    }
}

pub struct RepoListView {
    store: Entity<RepoListStore>,
    dock_area: WeakEntity<DockArea>,
    focus_handle: FocusHandle,
    scroll_handle: VirtualListScrollHandle,
    filter: RepoFilter,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    /// Number of rows [`Self::item_sizes`] was built for, the filtered repo count.
    repo_len: usize,
    /// Indices matching [`Self::filter`] into the store's `announcements`.
    visible: Vec<usize>,
    search: Entity<InputState>,
    _search_subscription: Subscription,
    _subscription: Subscription,
}

impl RepoListView {
    pub fn new(
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = RepoListStore::global(cx);

        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search..."));
        let search_subscription = cx.subscribe(&search, |this, _search, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.rebuild_rows(cx);
            }
        });

        // Keep the visible slice and row sizes in sync with the store.
        // Newly announced repositories appear without waiting for a click.
        let subscription = cx.observe(&store, |this, _store, cx| {
            this.rebuild_rows(cx);
        });

        cx.defer_in(window, |this, _window, cx| {
            this.rebuild_rows(cx);
        });

        Self {
            store,
            dock_area,
            focus_handle: cx.focus_handle(),
            scroll_handle: VirtualListScrollHandle::new(),
            filter: RepoFilter::default(),
            item_sizes: Rc::new(Vec::new()),
            repo_len: 0,
            visible: Vec::new(),
            search,
            _search_subscription: search_subscription,
            _subscription: subscription,
        }
    }

    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let filter = self.filter;
        let query = self.search.read(cx).value();
        let store = self.store.read(cx);

        self.visible = filter.visible(store, &query);

        // Each virtual list row holds `COLUMNS` repo cards.
        let rows = self.visible.len().div_ceil(COLUMNS);

        if self.repo_len != rows {
            self.repo_len = rows;
            self.item_sizes = Rc::new(vec![size(px(0.), px(CARD_HEIGHT)); rows]);
        }

        cx.notify();
    }

    fn open_repo(
        &mut self,
        announcement: &Announcement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        open_repo_panel(
            &self.dock_area,
            &announcement.addr(),
            Some(announcement),
            window,
            cx,
        );
    }

    fn render_card(
        &self,
        ix: usize,
        announcement: &Announcement,
        last_activity: Option<Timestamp>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let profile_store = ProfileStore::global(cx);
        let owner = profile_store.read(cx).get(&announcement.owner);

        let name = announcement
            .name
            .as_deref()
            .map(SharedString::from)
            .unwrap_or(SharedString::from(announcement.id.clone()));

        let description = announcement
            .description
            .as_deref()
            .map(SharedString::from)
            .unwrap_or(SharedString::from("No description"));

        let activity = last_activity
            .map(relative_time)
            .map(|label| SharedString::from(format!("Updated {label}")))
            .unwrap_or_default();

        // The fork badge shows the upstream name when its announcement is known locally.
        // Otherwise it shows the repository id from the `u` tag.
        let fork_label: Option<SharedString> =
            announcement.upstream.as_ref().and_then(|upstream| {
                let addr = upstream.addr.as_ref()?;
                let name = self
                    .store
                    .read(cx)
                    .announcements
                    .iter()
                    .find(|a| a.addr() == *addr)
                    .map(|a| {
                        a.name
                            .as_deref()
                            .map(SharedString::from)
                            .unwrap_or_else(|| SharedString::from(a.id.clone()))
                    })
                    .unwrap_or_else(|| SharedString::from(addr.identifier.clone()));
                Some(SharedString::from(format!("forked from {name}")))
            });

        v_flex()
            .id(ix)
            .flex_1()
            .h_full()
            .min_w_0()
            .px_3()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .hover(|this| this.bg(cx.theme().list_hover))
            .child(
                h_flex()
                    .h_10()
                    .gap_1p5()
                    .items_center()
                    .child(
                        div()
                            .min_w_0()
                            .text_sm()
                            .font_semibold()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(name),
                    )
                    .when_some(fork_label, |this, label| {
                        this.child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .whitespace_nowrap()
                                .child(Icon::new(CustomIconName::GitBranch).small())
                                .child(label),
                        )
                    }),
            )
            .child(
                div()
                    .h_16()
                    .min_w_0()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .line_clamp(2)
                    .text_ellipsis()
                    .child(description),
            )
            .child(
                h_flex()
                    .h_12()
                    .gap_2()
                    .items_center()
                    .overflow_hidden()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(UserAvatar::new(owner.name()).picture(owner.picture()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .whitespace_nowrap()
                                    .child(owner.name()),
                            ),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .justify_end()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .whitespace_nowrap()
                            .child(activity),
                    ),
            )
            .on_click(cx.listener({
                let announcement = announcement.clone();
                move |this, _ev, window, cx| {
                    this.open_repo(&announcement, window, cx);
                }
            }))
            .into_any_element()
    }

    fn render_filter<T>(&self, filter: RepoFilter, label: T, cx: &mut Context<Self>) -> AnyElement
    where
        T: Into<SharedString>,
    {
        let active = self.filter == filter;

        SegmentButton::new(filter.to_string(), label)
            .icon(Icon::new(filter.icon_name()))
            .selected(active)
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.filter = filter;
                this.rebuild_rows(cx);
            }))
            .into_any_element()
    }

    fn render_header(&self, count: usize, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .px_4()
            .py_2()
            .w_full()
            .gap_3()
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_ellipsis()
                            .font_semibold()
                            .text_xs()
                            .line_height(relative(1.2))
                            .child("Repositories"),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(cx.theme().muted_foreground)
                            .line_height(relative(1.2))
                            .child(SharedString::from(format!("Total: {count}"))),
                    ),
            )
            .child(
                Input::new(&self.search)
                    .cleanable(true)
                    .w(px(180.))
                    .text_sm()
                    .border_color(cx.theme().muted)
                    .bg(cx.theme().muted)
                    .prefix(Icon::new(IconName::Search).small()),
            )
            .child(div().flex_1())
            .child(
                h_flex()
                    .gap_1()
                    .child(self.render_filter(RepoFilter::All, "All", cx))
                    .child(self.render_filter(RepoFilter::Popular, "Popular", cx))
                    .child(self.render_filter(RepoFilter::Recent, "Recent", cx)),
            )
            .into_any_element()
    }
}

impl BasePanel for RepoListView {
    fn panel_name(&self) -> &'static str {
        "repo_list"
    }
}

impl Panel for RepoListView {
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().text_sm().child(SharedString::from("Explore"))
    }
}

impl EventEmitter<PanelEvent> for RepoListView {}

impl Focusable for RepoListView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RepoListView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let announcements = store.announcements.clone();
        let last_activity = store.last_activity.clone();
        let count = self.visible.len();
        let has_repos = count > 0;

        v_flex()
            .relative()
            .image_cache(gpui::retain_all("repos"))
            .size_full()
            .child(self.render_header(count, cx))
            .when(!has_repos, |this| {
                this.child(
                    v_flex().size_full().items_center().justify_center().child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("No repositories found yet."),
                    ),
                )
            })
            .when(has_repos, |this| {
                let view = cx.entity().clone();
                let sizes = self.item_sizes.clone();

                this.child(
                    v_virtual_list(view, "repos", sizes, move |this, range, _window, cx| {
                        let mut items = vec![];

                        for row in range {
                            let mut row_cards = vec![];

                            for col in 0..COLUMNS {
                                let Some(&ix) = this.visible.get(row * COLUMNS + col) else {
                                    break;
                                };
                                let Some(announcement) = announcements.get(ix) else {
                                    break;
                                };
                                let activity = last_activity.get(&announcement.addr()).copied();
                                row_cards.push(this.render_card(ix, announcement, activity, cx));
                            }

                            items.push(
                                h_flex()
                                    .id(row)
                                    .w_full()
                                    .h_full()
                                    .gap_3()
                                    .px_4()
                                    .pt_4()
                                    .children(row_cards)
                                    .into_any_element(),
                            );
                        }

                        items
                    })
                    .track_scroll(&self.scroll_handle)
                    .size_full(),
                )
            })
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .child(Scrollbar::vertical(&self.scroll_handle)),
            )
    }
}
