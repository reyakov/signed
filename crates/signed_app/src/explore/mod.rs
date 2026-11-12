mod helpers;

use assets::CustomIconName;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, UniformListScrollHandle, WeakEntity, Window,
    div, px, uniform_list,
};
use gpui_native_kit::common::{h_flex, v_flex};
use gpui_native_kit::empty_state::EmptyState;
use gpui_native_kit::icons::{Icon, IconRole};
use gpui_native_kit::input::{Input, InputState, InputStateEvent};
use gpui_native_kit::layout::dock::{DockState, Panel};
use gpui_native_kit::scrollbar::Scrollbar;
use gpui_native_kit::segmented::{Segmented, SegmentedOption};
use gpui_native_kit::theme::ActiveTheme as _;
use helpers::{RepoFilter, render_card};
use metadata::ProfileDirectory;
use nostr::prelude::PublicKey;
use signed_state::RepoListStore;

const COLUMNS: usize = 2;
const CARD_HEIGHT: f32 = 40. + 64. + 48. + 2. + 6.;

pub struct ExploreView {
    store: Entity<RepoListStore>,
    dock: WeakEntity<DockState>,
    scroll_handle: UniformListScrollHandle,
    filter: RepoFilter,
    /// Indices matching [`Self::filter`] into the store's `announcements`.
    visible: Vec<usize>,
    search: Entity<InputState>,
    /// Profiles for the card owners.
    directory: Entity<ProfileDirectory>,
    _search_subscription: Subscription,
    _store_subscription: Subscription,
}

impl ExploreView {
    pub fn new(dock: WeakEntity<DockState>, cx: &mut Context<Self>) -> Self {
        let store = RepoListStore::global(cx);
        let directory = ProfileDirectory::new(cx);

        let weak = cx.entity().downgrade();
        let search = InputState::new(cx);

        let search_subscription = cx.subscribe(&search, |this, _search, event, cx| {
            if matches!(event, InputStateEvent::TextChanged) {
                this.rebuild_rows(cx);
            }
        });

        let store_subscription = cx.observe(&store, |this, _store, cx| {
            this.rebuild_rows(cx);
        });

        cx.defer(move |cx| {
            if let Err(error) = weak.update(cx, |this, cx| this.rebuild_rows(cx)) {
                log::warn!("failed to load explore rows: {error}");
            }
        });

        Self {
            store,
            dock,
            scroll_handle: UniformListScrollHandle::new(),
            filter: RepoFilter::default(),
            visible: Vec::new(),
            search,
            directory,
            _search_subscription: search_subscription,
            _store_subscription: store_subscription,
        }
    }

    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let filter = self.filter;
        let query = self.search.read(cx).value().to_string();
        let store = self.store.read(cx);

        self.visible = filter.visible(store, &query);

        let owners: Vec<PublicKey> = self
            .visible
            .iter()
            .filter_map(|ix| store.announcements.get(*ix))
            .map(|announcement| announcement.owner)
            .collect();

        self.directory.update(cx, |directory, cx| {
            directory.track(owners, cx);
        });

        cx.notify();
    }

    fn render_header(&self, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().colors.muted_foreground;
        let weak = cx.entity().downgrade();

        h_flex()
            .px_4()
            .py_2()
            .w_full()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_xs()
                            .child("Repositories"),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(muted)
                            .child(SharedString::from(format!("Total: {count}"))),
                    ),
            )
            .child(
                div().w(px(180.)).child(
                    Input::new("explore-search", self.search.clone())
                        .placeholder("Search...")
                        .prefix(Icon::named(IconRole::Search).size(px(14.)).color(muted)),
                ),
            )
            .child(div().flex_1())
            .child(
                Segmented::new("explore-filter", self.filter)
                    .options(vec![
                        SegmentedOption::new(RepoFilter::All, "All")
                            .icon(Icon::named(CustomIconName::Grid).size(px(14.)).color(muted)),
                        SegmentedOption::new(RepoFilter::Popular, "Popular").icon(
                            Icon::named(CustomIconName::Trending)
                                .size(px(14.))
                                .color(muted),
                        ),
                        SegmentedOption::new(RepoFilter::Recent, "Recent").icon(
                            Icon::named(CustomIconName::Recent)
                                .size(px(14.))
                                .color(muted),
                        ),
                    ])
                    .on_change(move |filter, _, cx| {
                        if let Err(error) = weak.update(cx, |this, cx| {
                            this.filter = filter;
                            this.rebuild_rows(cx);
                        }) {
                            log::warn!("failed to change explore filter: {error}");
                        }
                    }),
            )
            .into_any_element()
    }
}

impl Panel for ExploreView {
    fn title(&self, _cx: &App) -> Option<SharedString> {
        Some(SharedString::from("Explore"))
    }

    fn lead(&self, cx: &App) -> Option<AnyElement> {
        Some(
            Icon::named(CustomIconName::Globe)
                .size(px(14.))
                .color(cx.theme().colors.muted_foreground)
                .into_any_element(),
        )
    }
}

impl Render for ExploreView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let announcements = store.announcements.clone();
        let last_activity = store.last_activity.clone();
        let count = self.visible.len();
        let visible = self.visible.clone();
        let directory = self.directory.clone();
        let dock = self.dock.clone();
        let has_repos = count > 0;
        let rows = visible.len().div_ceil(COLUMNS);

        v_flex()
            .relative()
            .size_full()
            .child(self.render_header(count, cx))
            .when(!has_repos, |this| {
                this.child(
                    v_flex()
                        .size_full()
                        .items_center()
                        .justify_center()
                        .child(EmptyState::new("No repositories match your search")),
                )
            })
            .when(has_repos, |this| {
                this.child(
                    uniform_list("explore-repos", rows, move |range, _, cx| {
                        range
                            .map(|row| {
                                let mut cards = Vec::with_capacity(COLUMNS);

                                for col in 0..COLUMNS {
                                    let Some(&ix) = visible.get(row * COLUMNS + col) else {
                                        break;
                                    };
                                    let Some(announcement) = announcements.get(ix) else {
                                        break;
                                    };
                                    let activity = last_activity.get(&announcement.addr()).copied();
                                    cards.push(render_card(
                                        ix,
                                        announcement,
                                        activity,
                                        &announcements,
                                        &directory,
                                        &dock,
                                        cx,
                                    ));
                                }

                                h_flex()
                                    .id(row)
                                    .w_full()
                                    .h(px(CARD_HEIGHT))
                                    .gap_3()
                                    .px_4()
                                    .pt_4()
                                    .children(cards)
                                    .into_any_element()
                            })
                            .collect()
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
                    .child(Scrollbar::vertical(
                        "explore-scroll",
                        self.scroll_handle.clone(),
                    )),
            )
    }
}
