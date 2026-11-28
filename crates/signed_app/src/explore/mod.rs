mod helpers;

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ListAlignment, ListState,
    ObjectFit, ParentElement as _, Pixels, Render, SharedString, Styled as _, StyledImage as _,
    Subscription, WeakEntity, Window, div, img, list, px,
};
use helpers::{CardData, RepoCard, RepoFilter};
use metadata::ProfileDirectory;
use nostr::prelude::PublicKey;
use signed_state::{Backend, RepoListStore};
use ui::button::{Button, ButtonVariant};
use ui::common::{h_flex, v_flex};
use ui::empty_state::EmptyState;
use ui::icons::{Icon, IconRole};
use ui::input::{Input, InputState, InputStateEvent};
use ui::layout::panel::{Panel, PanelSlot, PanelStack, SplitSide};
use ui::scrollbar::Scrollbar;
use ui::segmented::{Segmented, SegmentedOption};
use ui::theme::ActiveTheme as _;

use crate::app::AppRoot;
use crate::shared::log_dropped;

const COLUMNS: usize = 2;
// Name row, avatar strip, meta row, and their borders and gaps, in px.
const CARD_HEIGHT: f32 = 40. + 64. + 48. + 2. + 6.;

/// Applies a new explore filter.
type FilterHandler = Rc<dyn Fn(RepoFilter, &mut Window, &mut App)>;

pub struct ExploreView {
    store: Entity<RepoListStore>,
    workspace: WeakEntity<AppRoot>,
    scroll_handle: ListState,
    filter: RepoFilter,
    /// Indices matching [`Self::filter`] into the store's `announcements`.
    visible: Vec<usize>,
    /// The visible cards' display strings, rebuilt with `visible`.
    cards: Arc<Vec<CardData>>,
    search: Entity<InputState>,
    /// Profiles for the card owners.
    directory: Entity<ProfileDirectory>,
    _search_subscription: Subscription,
    _store_subscription: Subscription,
    /// Observes the panel stack for the onboarding sidebar, set once this view is in it.
    _stack_subscription: Option<Subscription>,
}

impl ExploreView {
    /// The identifier of the explore panel in the workspace; shown as its tab label.
    pub(crate) const PANEL_ID: &'static str = "Explore";

    /// Opens the explore panel in the main panel, reusing it when it is already cached.
    pub(crate) fn open(
        panels: &Entity<PanelStack>,
        workspace: WeakEntity<AppRoot>,
        window: &mut Window,
        cx: &mut App,
    ) {
        panels.update(cx, |panels, cx| {
            panels.open(
                PanelSlot::Main,
                Self::PANEL_ID,
                move |_, cx| cx.new(|cx| Self::new(workspace.clone(), cx)),
                window,
                cx,
            );
        });
    }

    pub fn new(workspace: WeakEntity<AppRoot>, cx: &mut Context<Self>) -> Self {
        let store = RepoListStore::global(cx);
        let directory = ProfileDirectory::new(cx);

        let weak = cx.weak_entity();
        let search = InputState::new(cx);

        let search_subscription = cx.subscribe(&search, |this, _search, event, cx| {
            if matches!(event, InputStateEvent::TextChanged) {
                this.rebuild_rows(false, cx);
            }
        });

        let store_subscription = cx.observe(&store, |this, _store, cx| {
            this.rebuild_rows(true, cx);
        });

        // The root and the stack are still being updated while this view is built.
        cx.defer(move |cx| {
            if let Err(error) = weak.update(cx, |this, cx| {
                this.rebuild_rows(false, cx);
                this.watch_stack(cx);
            }) {
                log::warn!("failed to set up the explore panel: {error}");
            }
        });

        Self {
            store,
            workspace,
            scroll_handle: ListState::new(1, ListAlignment::Top, px(320.)),
            filter: RepoFilter::default(),
            visible: Vec::new(),
            cards: Arc::new(Vec::new()),
            search,
            directory,
            _search_subscription: search_subscription,
            _store_subscription: store_subscription,
            _stack_subscription: None,
        }
    }

    /// `anchor` keeps the scroll position while the row count is unchanged.
    fn rebuild_rows(&mut self, anchor: bool, cx: &mut Context<Self>) {
        let filter = self.filter;
        let query = self.search.read(cx).value().to_string();
        let store = self.store.read(cx);

        self.visible = filter.visible(store, &query);
        self.cards = Arc::new(
            self.visible
                .iter()
                .filter_map(|ix| {
                    let announcement = store.announcements.get(*ix)?;
                    let activity = store.last_activity.get(&announcement.addr()).copied();
                    Some(CardData::new(
                        *ix,
                        announcement,
                        &store.announcements,
                        activity,
                    ))
                })
                .collect(),
        );

        let owners: Vec<PublicKey> = self
            .visible
            .iter()
            .filter_map(|ix| store.announcements.get(*ix))
            .map(|announcement| announcement.owner)
            .collect();

        self.directory.update(cx, |directory, cx| {
            directory.track(owners, cx);
        });

        let count = self.item_count();
        if !anchor || count != self.scroll_handle.item_count() {
            self.scroll_handle.reset(count);
        }

        cx.notify();
    }

    /// Header row, plus one row per card row, or the empty state when nothing matches.
    fn item_count(&self) -> usize {
        if self.visible.is_empty() {
            2
        } else {
            self.visible.len().div_ceil(COLUMNS) + 1
        }
    }

    /// Observes the panel stack so the onboarding sidebar follows the shown panel.
    fn watch_stack(&mut self, cx: &mut Context<Self>) {
        let Ok(stack) = self
            .workspace
            .read_with(cx, |workspace, _| workspace.panels().clone())
        else {
            log::warn!("failed to watch the panel stack: the app root is gone");
            return;
        };

        self._stack_subscription = Some(cx.observe(&stack, |this, _stack, cx| {
            this.sync_sidebar(cx);
        }));
        self.sync_sidebar(cx);
    }

    /// Shows the onboarding sidebar while signed out and an explore panel is shown, and closes it otherwise.
    fn sync_sidebar(&mut self, cx: &mut Context<Self>) {
        let signed_out = Backend::global(cx).read(cx).current_user().is_none();
        let explore = SharedString::from(Self::PANEL_ID);
        let sidebar = SharedString::from(OnboardingSidebar::PANEL_ID);

        let result = self.workspace.update(cx, |workspace, cx| {
            workspace.panels().clone().update(cx, |stack, cx| {
                let shown = signed_out && stack.is_active(PanelSlot::Main, &explore);

                if shown && !stack.is_open(PanelSlot::Split, &sidebar) {
                    let view = Arc::new(cx.new(|_| OnboardingSidebar {
                        workspace: self.workspace.clone(),
                    }));
                    stack.insert_split_at(
                        sidebar.clone(),
                        OnboardingSidebar::WIDTH,
                        SplitSide::Leading,
                        view,
                        cx,
                    );
                } else if !shown && stack.is_open(PanelSlot::Split, &sidebar) {
                    stack.close(PanelSlot::Split, &sidebar, cx);
                }
            })
        });

        if let Err(error) = result {
            log::warn!("failed to update the onboarding sidebar: {error}");
        }
    }

    /// One list row: the search header, or one strip of repository cards.
    fn render_list_item(&mut self, item: usize, cx: &mut Context<Self>) -> AnyElement {
        if item == 0 {
            let on_filter: FilterHandler =
                Rc::new(cx.processor(|this, filter: RepoFilter, _, cx| {
                    this.filter = filter;
                    this.rebuild_rows(false, cx);
                }));
            return Self::render_header(self.search.clone(), self.filter, on_filter, cx)
                .into_any_element();
        }

        if self.visible.is_empty() {
            return v_flex()
                .w_full()
                .h(px(320.))
                .items_center()
                .justify_center()
                .child(EmptyState::new("No repositories match your search"))
                .into_any_element();
        }

        let store = self.store.read(cx);
        let row = item - 1;
        let mut rendered = Vec::with_capacity(COLUMNS);

        for col in 0..COLUMNS {
            let Some(data) = self.cards.get(row * COLUMNS + col) else {
                break;
            };
            let Some(announcement) = store.announcements.get(data.ix) else {
                break;
            };
            rendered.push(RepoCard::new(
                data,
                announcement,
                &self.directory,
                &self.workspace,
            ));
        }

        h_flex()
            .w_full()
            .h(px(CARD_HEIGHT))
            .gap_3()
            .px_4()
            .pt_4()
            .children(rendered)
            .into_any_element()
    }

    fn render_header(
        search: Entity<InputState>,
        filter: RepoFilter,
        on_filter: FilterHandler,
        cx: &App,
    ) -> impl IntoElement {
        let muted = cx.theme().colors.muted_foreground;

        div()
            .relative()
            .h_96()
            .w_full()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px_4()
                    .child(
                        div().w_full().max_w(px(480.)).child(
                            Input::new("explore-search", search)
                                .placeholder("Search...")
                                .fully_rounded(true)
                                .prefix(Icon::named(IconRole::Search).small().color(muted)),
                        ),
                    ),
            )
            .child(
                div().absolute().left_0().bottom_0().pl_4().pb_4().child(
                    Segmented::new("explore-filter", filter)
                        .options(vec![
                            SegmentedOption::new(RepoFilter::All)
                                .label("All")
                                .icon(Icon::named(IconRole::Grid).small().color(muted)),
                            SegmentedOption::new(RepoFilter::Recent)
                                .label("Recent")
                                .icon(Icon::named(IconRole::Clock).small().color(muted)),
                        ])
                        .on_change(move |filter, window, cx| {
                            on_filter(filter, window, cx);
                        }),
                ),
            )
    }
}

impl Panel for ExploreView {
    fn title(&self, _cx: &App) -> SharedString {
        SharedString::from("Explore")
    }

    fn prefix(&self, cx: &App) -> Option<AnyElement> {
        Some(
            Icon::named(IconRole::Grid)
                .color(cx.theme().colors.foreground)
                .into_any_element(),
        )
    }
}

impl Render for ExploreView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scroll_handle = self.scroll_handle.clone();

        div()
            .relative()
            .size_full()
            .child(
                list(
                    self.scroll_handle.clone(),
                    cx.processor(|this, item: usize, _window, cx| this.render_list_item(item, cx)),
                )
                .size_full(),
            )
            .child(Scrollbar::vertical("explore-scroll", scroll_handle))
    }
}

/// The onboarding sidebar shown beside the explore panel while signed out.
struct OnboardingSidebar {
    workspace: WeakEntity<AppRoot>,
}

impl OnboardingSidebar {
    /// The identifier of the sidebar in the panel stack; shown as its tab label.
    const PANEL_ID: &'static str = "Onboarding";
    /// The width the sidebar opens at.
    const WIDTH: Pixels = px(300.);

    /// One of the three banner photos, picked per process launch.
    fn banner() -> SharedString {
        static BANNER: std::sync::LazyLock<SharedString> = std::sync::LazyLock::new(|| {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|now| now.subsec_nanos())
                .unwrap_or(0);
            SharedString::from(format!("backgrounds/banner{}.jpg", nanos % 3 + 1))
        });
        BANNER.clone()
    }
}

impl Panel for OnboardingSidebar {
    fn title(&self, _cx: &App) -> SharedString {
        SharedString::from(Self::PANEL_ID)
    }
}

impl Render for OnboardingSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let colors = theme.colors;
        let workspace = self.workspace.clone();

        v_flex()
            .size_full()
            .relative()
            .bg(colors.background)
            .rounded(theme.radius.overlay)
            .overflow_hidden()
            .child(
                div().absolute().inset_0().child(
                    img(OnboardingSidebar::banner())
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .rounded(theme.radius.overlay),
                ),
            )
            .child(
                v_flex()
                    .size_full()
                    .justify_end()
                    .p_4()
                    .gap_6()
                    .child(img("backgrounds/headline.png").max_w_48())
                    .child(
                        v_flex()
                            .gap_2()
                            .w_full()
                            .child(
                                Button::new("join")
                                    .label("Join now")
                                    .variant(ButtonVariant::Primary)
                                    .on_click({
                                        let workspace = workspace.clone();
                                        move |_, _, cx| {
                                            log_dropped(workspace.update(cx, |root, cx| {
                                                root.open_onboarding_dialog(cx)
                                            }));
                                        }
                                    }),
                            )
                            .child(
                                Button::new("import")
                                    .label("Import identity")
                                    .variant(ButtonVariant::Secondary)
                                    .on_click({
                                        let workspace = workspace.clone();
                                        move |_, window, cx| {
                                            log_dropped(workspace.update(cx, |root, cx| {
                                                root.open_import_dialog(window, cx)
                                            }));
                                        }
                                    }),
                            ),
                    ),
            )
    }
}
