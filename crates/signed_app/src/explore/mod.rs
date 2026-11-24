use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ListAlignment, ListState,
    ObjectFit, ParentElement as _, Pixels, Render, SharedString, Styled as _, StyledImage as _,
    Subscription, WeakEntity, Window, div, img, list, px,
};
use helpers::{RepoFilter, render_card};
use metadata::ProfileDirectory;
use nostr::prelude::PublicKey;
use signed_state::{Backend, RepoListStore};
use ui::button::{Button, ButtonVariant};
use ui::common::{h_flex, v_flex};
use ui::empty_state::EmptyState;
use ui::icons::{Icon, IconRole};
use ui::input::{Input, InputState, InputStateEvent};
use ui::layout::panel::{Panel, PanelSlot, SplitSide};
use ui::layout::root::RootContext as _;
use ui::scrollbar::Scrollbar;
use ui::segmented::{Segmented, SegmentedOption};
use ui::theme::ActiveTheme as _;

use crate::app::AppRoot;
use crate::explore::helpers::pick_banner;
use crate::identity::import::ImportDialog;
use crate::identity::onboarding::OnboardingDialog;
use crate::shared::panels;

mod helpers;

const COLUMNS: usize = 2;
// Name row, avatar strip, meta row, and their borders and gaps, in px.
const CARD_HEIGHT: f32 = 40. + 64. + 48. + 2. + 6.;

pub struct ExploreView {
    store: Entity<RepoListStore>,
    workspace: WeakEntity<AppRoot>,
    scroll_handle: ListState,
    filter: RepoFilter,
    /// Indices matching [`Self::filter`] into the store's `announcements`.
    visible: Vec<usize>,
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
    pub(crate) fn open(workspace: &WeakEntity<AppRoot>, window: &mut Window, cx: &mut App) {
        panels::open_primary(
            workspace,
            Self::PANEL_ID,
            {
                let workspace = workspace.clone();
                move |_, cx| cx.new(|cx| Self::new(workspace.clone(), cx))
            },
            window,
            cx,
        );
    }

    pub fn new(workspace: WeakEntity<AppRoot>, cx: &mut Context<Self>) -> Self {
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

        // The root and the stack are still being updated while this view is built.
        cx.defer(move |cx| {
            if let Err(error) = weak.update(cx, |this, cx| {
                this.rebuild_rows(cx);
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
            search,
            directory,
            _search_subscription: search_subscription,
            _store_subscription: store_subscription,
            _stack_subscription: None,
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

        self.scroll_handle.reset(self.item_count());
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
        self._stack_subscription =
            Some(cx.observe(&stack, |this, _stack, cx| this.sync_sidebar(cx)));
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
                    let view = Arc::new(cx.new(|_| OnboardingSidebar));
                    stack.insert_split_at(
                        sidebar.clone(),
                        OnboardingSidebar::WIDTH,
                        SplitSide::Leading,
                        view,
                        cx,
                    );
                } else if !shown {
                    stack.close(PanelSlot::Split, &sidebar, cx);
                }
            })
        });

        if let Err(error) = result {
            log::warn!("failed to update the onboarding sidebar: {error}");
        }
    }

    fn render_header(
        search: Entity<InputState>,
        filter: RepoFilter,
        weak: WeakEntity<Self>,
        cx: &App,
    ) -> AnyElement {
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
                            SegmentedOption::new(RepoFilter::All, "All")
                                .icon(Icon::named(IconRole::Grid).small().color(muted)),
                            SegmentedOption::new(RepoFilter::Recent, "Recent")
                                .icon(Icon::named(IconRole::Clock).small().color(muted)),
                        ])
                        .on_change(move |filter, _, cx| {
                            if let Err(error) = weak.update(cx, |this, cx| {
                                this.filter = filter;
                                this.rebuild_rows(cx);
                            }) {
                                log::warn!("failed to change explore filter: {error}");
                            }
                        }),
                ),
            )
            .into_any_element()
    }
}

impl Panel for ExploreView {
    fn title(&self, _cx: &App) -> SharedString {
        SharedString::from("Explore")
    }

    fn prefix(&self, cx: &App) -> Option<AnyElement> {
        Some(
            Icon::named(IconRole::Grid)
                .small()
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
        let visible = self.visible.clone();
        let directory = self.directory.clone();
        let workspace = self.workspace.clone();
        let has_repos = !visible.is_empty();
        let search = self.search.clone();
        let filter = self.filter;
        let weak = cx.entity().downgrade();
        let scroll_handle = self.scroll_handle.clone();

        div()
            .relative()
            .size_full()
            .child(
                list(self.scroll_handle.clone(), move |item, _, cx| {
                    if item == 0 {
                        return Self::render_header(search.clone(), filter, weak.clone(), cx);
                    }

                    if !has_repos {
                        return v_flex()
                            .w_full()
                            .h(px(320.))
                            .items_center()
                            .justify_center()
                            .child(EmptyState::new("No repositories match your search"))
                            .into_any_element();
                    }

                    let row = item - 1;
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
                            &workspace,
                            cx,
                        ));
                    }

                    h_flex()
                        .w_full()
                        .h(px(CARD_HEIGHT))
                        .gap_3()
                        .px_4()
                        .pt_4()
                        .children(cards)
                        .into_any_element()
                })
                .size_full(),
            )
            .child(Scrollbar::vertical("explore-scroll", scroll_handle))
    }
}

/// The onboarding sidebar shown beside the explore panel while signed out.
struct OnboardingSidebar;

impl OnboardingSidebar {
    /// The identifier of the sidebar in the panel stack; shown as its tab label.
    const PANEL_ID: &'static str = "Onboarding";
    /// The width the sidebar opens at.
    const WIDTH: Pixels = px(300.);
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

        v_flex()
            .size_full()
            .relative()
            .bg(colors.background)
            .rounded(theme.radius.overlay)
            .overflow_hidden()
            .child(
                div().absolute().inset_0().child(
                    img(pick_banner())
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
                                    .on_click(|_, _, cx| {
                                        let dialog = OnboardingDialog::new(cx);
                                        cx.open_dialog(dialog)
                                    }),
                            )
                            .child(
                                Button::new("import")
                                    .label("Import identity")
                                    .variant(ButtonVariant::Secondary)
                                    .on_click(|_, _, cx| {
                                        let dialog = ImportDialog::new(cx);
                                        cx.open_dialog(dialog)
                                    }),
                            ),
                    ),
            )
    }
}
