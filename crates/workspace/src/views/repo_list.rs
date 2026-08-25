use std::rc::Rc;

use dock::{BasePanel, DockArea, DockPlacement, Panel, PanelEvent, panel_handle};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels, Render,
    SharedString, Size, Subscription, WeakEntity, Window, div, px, size,
};
use gpui_component::avatar::Avatar;
use gpui_component::scroll::Scrollbar;
use gpui_component::{
    ActiveTheme, Sizable, StyledExt, VirtualListScrollHandle, h_flex, v_flex, v_virtual_list,
};
use signed_core::Announcement;
use signed_state::{ProfileStore, RepoListStore, Timestamp};
use utils::relative_time;

use super::RepoDetailView;
use crate::image_cache::{MAX_IMAGES, image_cache};

const CARD_HEIGHT: f32 = 160.;

/// Browse all announced repositories (works anonymously).
pub struct RepoListView {
    store: Entity<RepoListStore>,
    dock_area: WeakEntity<DockArea>,
    focus_handle: FocusHandle,
    scroll_handle: VirtualListScrollHandle,
    item_sizes: Rc<Vec<Size<Pixels>>>,
    _subscription: Subscription,
}

impl RepoListView {
    pub fn new(
        dock_area: WeakEntity<DockArea>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let store = cx.new(|cx| RepoListStore::new(None, cx));

        let subscription = cx.observe(&store, |this, store, cx| {
            let count = store.read(cx).announcements.len();

            if this.item_sizes.len() != count {
                this.item_sizes = Rc::new(vec![size(px(0.), px(CARD_HEIGHT)); count]);
            }
        });

        Self {
            store,
            dock_area,
            focus_handle: cx.focus_handle(),
            scroll_handle: VirtualListScrollHandle::new(),
            item_sizes: Rc::new(vec![]),
            _subscription: subscription,
        }
    }

    fn open_repo(
        &mut self,
        announcement: &Announcement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dock_area = self.dock_area.clone();
        let detail =
            cx.new(|cx| RepoDetailView::new(dock_area.clone(), announcement.clone(), window, cx));

        if let Some(dock_area) = dock_area.upgrade() {
            dock_area.update(cx, |dock_area, cx| {
                dock_area.add_panel_view(
                    panel_handle(detail),
                    DockPlacement::Center,
                    None,
                    window,
                    cx,
                );
            });
        }
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
            .clone()
            .unwrap_or_else(|| SharedString::from(announcement.id.clone()));

        let description = announcement
            .description
            .clone()
            .unwrap_or(SharedString::from("No description"));

        let activity = last_activity
            .map(relative_time)
            .map(|label| SharedString::from(format!("Updated {label}")))
            .unwrap_or_default();

        v_flex()
            .id(ix)
            .px_4()
            .w_full()
            .border_b(px(1.))
            .border_color(cx.theme().border)
            .hover(|this| this.bg(cx.theme().list_hover))
            .child(
                h_flex()
                    .h_12()
                    .text_sm()
                    .font_semibold()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(name),
            )
            .child(
                div()
                    .h_16()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .line_clamp(2)
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
                            .child(
                                Avatar::new()
                                    .name(owner.name())
                                    .when_some(owner.picture(), |this, url| this.src(url))
                                    .rounded(cx.theme().radius)
                                    .small(),
                            )
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
        let announcements = self.store.read(cx).announcements.clone();
        let last_activity = self.store.read(cx).last_activity.clone();
        let has_announcements = !announcements.is_empty();
        let count = announcements.len();

        v_flex()
            .relative()
            .image_cache(image_cache("repos", MAX_IMAGES))
            .size_full()
            .child(
                h_flex()
                    .px_4()
                    .py_2()
                    .items_center()
                    .child(div().text_sm().font_semibold().child("Repositories"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(format!(" ({count})"))),
                    ),
            )
            .when(!has_announcements, |this| {
                this.child(
                    v_flex().size_full().items_center().justify_center().child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("No repositories found. Waiting for relays..."),
                    ),
                )
            })
            .when(has_announcements, |this| {
                let view = cx.entity().clone();
                let sizes = self.item_sizes.clone();

                this.child(
                    v_virtual_list(view, "repos", sizes, move |this, range, _window, cx| {
                        let mut items = vec![];

                        for ix in range {
                            let announcement: &Announcement = &announcements[ix];
                            let activity = last_activity.get(&announcement.addr()).copied();
                            items.push(this.render_card(ix, announcement, activity, cx));
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
