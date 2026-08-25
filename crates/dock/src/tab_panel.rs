//! The Signed appearance for a tab group.
//!
//! `gpui_base::dock::TabGroup` owns the behavior — membership, the displayed
//! tab, drag hit-testing, the zoom flag — and draws none of it. Everything
//! visible is here, ported from the vendored dock: the pill tab bar that
//! doubles as the window title bar (with window controls, title-bar
//! dragging, and previous/next tab buttons), the toolbar, the ellipsis menu,
//! the dock collapse affordance, the drop placeholder, and the styled drag
//! preview.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Anchor, Animation, AnimationExt as _, AnyElement, AnyView, App, AppContext as _, Bounds,
    Context, Div, Empty, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point,
    Render, ScrollHandle, SharedString, Stateful, StatefulInteractiveElement as _, StyleRefinement,
    Styled as _, Window, div, px, size,
};
use gpui_base::dock::{
    AnyDrag, DockPlacement, DragPanel, DropIndicator, NodeId, PaneNode, PaneRef,
    PanelView as BasePanelView, TabGroupContext, TabGroupRenderer,
};
use gpui_base::{ElementExt, InteractiveElementExt, Tab, Tabs};
use gpui_component::animation::{Lerp as _, ease_out_cubic};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::menu::DropdownMenu as _;
use gpui_component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, h_flex, v_flex,
};

use crate::dock_area::SkinShared;
use crate::{
    ClosePanel, PanelControl, PanelHandle, TAB_BAR_HEIGHT, ToggleZoom, t, title_bar_drag_handlers,
    window_controls,
};

/// The size the styled drag preview occupies, reported to base so a drop
/// placeholder knows where to fly in from.
const DRAG_PREVIEW_SIZE: gpui::Size<gpui::Pixels> = size(px(96.), px(30.));

/// A panel's title, or its registered name when it reached base without this
/// crate's handle and so carries no presentation. See [`PanelHandle::of`].
pub(crate) fn panel_title(
    panel: &Arc<dyn BasePanelView>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    match PanelHandle::of(panel) {
        Some(handle) => handle.title(window, cx),
        None => SharedString::from(panel.panel_name(cx)).into_any_element(),
    }
}

/// The preview that follows the cursor while a panel is dragged.
///
/// `gpui_base::dock::DragPanel` is the payload and draws nothing; this is the
/// appearance half, reintroduced here.
struct DragPanelPreview {
    panel: Arc<dyn BasePanelView>,
}

impl Render for DragPanelPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("drag-panel")
            .cursor_grab()
            .py_1()
            .px_3()
            .w_24()
            .overflow_hidden()
            .whitespace_nowrap()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius)
            .text_color(cx.theme().tab_foreground)
            .bg(cx.theme().tokens.tab_active)
            .opacity(0.75)
            .child(panel_title(&self.panel, window, cx))
    }
}

/// Where the zoom affordance goes for the group's displayed panel, or `None`
/// when there is none to offer.
///
/// Two questions, and both have to be asked. [`Panel::zoom_control`] says
/// *where* the control appears; [`gpui_base::dock::Panel::zoomable`] says
/// whether zooming happens at all, and base refuses a zoom that fails it.
fn zoom_control(group: &TabGroupContext, cx: &App) -> Option<PanelControl> {
    let panel = group.active_panel()?;
    panel
        .zoomable(cx)
        .then(|| PanelHandle::of(panel).and_then(|handle| handle.zoom_control(cx)))
        .flatten()
}

/// The left-most, top-most tab group in a container — where a left dock's
/// collapse affordance goes. Mirrors the old `StackPanel::left_top_tab_panel`.
fn left_top_group(node: &PaneNode) -> Option<NodeId> {
    match node.kind() {
        PaneRef::Tabs { .. } => Some(node.id()),
        PaneRef::Split { children, .. } => children.first().and_then(left_top_group),
        PaneRef::Tiles { .. } => None,
    }
}

/// The right-most, top-most tab group. A vertical split stacks its children,
/// so its *first* child is the top one; a horizontal split's last child is
/// the right-most. Mirrors the old `StackPanel::right_top_tab_panel`.
fn right_top_group(node: &PaneNode) -> Option<NodeId> {
    match node.kind() {
        PaneRef::Tabs { .. } => Some(node.id()),
        PaneRef::Split { axis, children, .. } => match axis {
            gpui::Axis::Vertical => children.first(),
            gpui::Axis::Horizontal => children.last(),
        }
        .and_then(right_top_group),
        PaneRef::Tiles { .. } => None,
    }
}

/// One tab group's appearance.
///
/// Built per group — `DockAreaRenderer::tab_group_renderer` is called once
/// per container — so the tab bar's scroll position and the measured
/// title-bar geometry belong to the group they describe.
pub(crate) struct SignedTabGroupSkin {
    shared: Rc<SkinShared>,
    scroll_handle: ScrollHandle,
    /// The displayed tab the last frame drew, so a change scrolls the new tab
    /// into view.
    last_active_ix: Cell<Option<usize>>,
    /// Bounds of the title bar row (the wrapper around the tab bar), in
    /// window coordinates. Measured via `on_prepaint` to position the
    /// title-bar drag overlay.
    title_bar_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Bounds of the tab bar's trailing empty space (right after the last
    /// tab), which marks where the draggable region starts.
    title_bar_strip_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// Bounds of the tab bar's suffix (toolbar) area, which marks where the
    /// draggable region ends.
    title_bar_suffix_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl SignedTabGroupSkin {
    pub(crate) fn new(shared: Rc<SkinShared>) -> Self {
        Self {
            shared,
            scroll_handle: ScrollHandle::default(),
            last_active_ix: Cell::new(None),
            title_bar_bounds: Rc::new(Cell::new(None)),
            title_bar_strip_bounds: Rc::new(Cell::new(None)),
            title_bar_suffix_bounds: Rc::new(Cell::new(None)),
        }
    }

    /// A group that is the left dock's whole content with a single panel
    /// draws no chrome at all — the vendored dock rendered such a panel bare,
    /// and the sidebar is one.
    fn is_plain_sidebar_group(&self, group: &TabGroupContext, cx: &mut App) -> bool {
        let Some(area) = self.shared.area().upgrade() else {
            return false;
        };
        let area = area.read(cx);
        let Some(left) = area
            .layout(DockPlacement::Left)
            .map(|tree| tree.root().id())
        else {
            return false;
        };
        left == group.node() && group.panels().len() == 1
    }

    /// The bottom or right dock whose root tab group this group is, if any.
    ///
    /// Base bars a dock's only group from being dragged or closed, so the
    /// dock cannot be emptied. A bottom/right panel is supposed to be
    /// closable and movable, though — the vendored dock allowed exactly that
    /// — so the skin recognizes the group and routes around the bar.
    fn is_dock_root_group(&self, group: &TabGroupContext, cx: &App) -> Option<DockPlacement> {
        let area = self.shared.area().upgrade()?;
        let area = area.read(cx);
        [DockPlacement::Bottom, DockPlacement::Right]
            .into_iter()
            .find(|placement| {
                area.layout(*placement)
                    .is_some_and(|tree| tree.root().id() == group.node())
            })
    }

    /// The drag payload for the tab at `ix`, or `None` when this group must
    /// not be rearranged. A locked group is never draggable; a group that is
    /// a bottom/right dock's only content still is, because the center is
    /// always there to land in.
    fn tab_drag(&self, group: &TabGroupContext, ix: usize, cx: &App) -> Option<DragPanel> {
        if group.is_locked() {
            return None;
        }
        if !group.is_draggable() && self.is_dock_root_group(group, cx).is_none() {
            return None;
        }
        group.drag_panel(ix, cx)
    }

    /// Whether a dock's collapse affordance belongs in *this* group's tab
    /// bar, and which way it points. `None` means this group draws none.
    fn dock_toggle_button(
        &self,
        placement: DockPlacement,
        group: &TabGroupContext,
        cx: &mut App,
    ) -> Option<Button> {
        if group.is_zoomed() || !self.shared.is_toggle_button_visible() {
            return None;
        }

        let area = self.shared.area().upgrade()?;
        let area = area.read(cx);
        // A dock that does not exist is not collapsible, so this covers the
        // old `left_dock.is_some()` test too.
        if !area.is_dock_collapsible(placement) {
            return None;
        }

        let designated = match placement {
            DockPlacement::Left => area
                .layout(DockPlacement::Center)
                .and_then(|tree| left_top_group(tree.root())),
            DockPlacement::Right => area
                .layout(DockPlacement::Center)
                .and_then(|tree| right_top_group(tree.root())),
            DockPlacement::Bottom => area
                .layout(DockPlacement::Bottom)
                .and_then(|tree| left_top_group(tree.root())),
            DockPlacement::Center => None,
        };
        if designated != Some(group.node()) {
            return None;
        }

        let is_open = area.is_dock_open(placement);
        let icon = match (placement, is_open) {
            (DockPlacement::Left, true) => IconName::PanelLeft,
            (DockPlacement::Left, false) => IconName::PanelLeftOpen,
            (DockPlacement::Right, true) => IconName::PanelRight,
            (DockPlacement::Right, false) => IconName::PanelRightOpen,
            (DockPlacement::Bottom, true) => IconName::PanelBottom,
            (DockPlacement::Bottom, false) => IconName::PanelBottomOpen,
            (DockPlacement::Center, _) => return None,
        };

        let area = self.shared.area().clone();
        Some(
            Button::new(SharedString::from(format!("toggle-dock:{placement:?}")))
                .icon(icon)
                .small()
                .ghost()
                .tab_stop(false)
                .tooltip(match is_open {
                    true => t("Dock.Collapse"),
                    false => t("Dock.Expand"),
                })
                .on_click(move |_, window, cx| {
                    _ = area.update(cx, |area, cx| area.toggle_dock(placement, window, cx));
                }),
        )
    }

    /// The previous/next tab buttons shown in the tab bar's leading prefix.
    ///
    /// Unlike the dock toggle button they always render, but are disabled at
    /// the ends of the tab strip (or while the panel is collapsed).
    fn render_prev_next_tab_buttons(
        &self,
        group: &TabGroupContext,
        _cx: &mut App,
    ) -> impl IntoElement {
        let collapsed = group.is_collapsed();
        let active_ix = group.active_ix();
        let panels_len = group.panels().len();
        let prev_enabled = !collapsed && active_ix > 0;
        let next_enabled = !collapsed && active_ix + 1 < panels_len;

        h_flex()
            .gap_1()
            .child(
                Button::new("tab:prev")
                    .icon(IconName::ArrowLeft)
                    .small()
                    .ghost()
                    .tab_stop(false)
                    .tooltip("Previous tab")
                    .disabled(!prev_enabled)
                    .on_click({
                        let group = group.clone();
                        move |_, window, cx| group.select_tab(active_ix - 1, window, cx)
                    }),
            )
            .child(
                Button::new("tab:next")
                    .icon(IconName::ArrowRight)
                    .small()
                    .ghost()
                    .tab_stop(false)
                    .tooltip("Next tab")
                    .disabled(!next_enabled)
                    .on_click({
                        let group = group.clone();
                        move |_, window, cx| group.select_tab(active_ix + 1, window, cx)
                    }),
            )
    }

    /// The trailing controls: the panel's own buttons, the zoom affordance,
    /// and the ellipsis menu.
    fn render_toolbar(
        &self,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        if group.is_collapsed() {
            return div();
        }

        let zoomed = group.is_zoomed();
        let handle = group.active_panel().and_then(PanelHandle::of);
        let control = zoom_control(group, cx);
        let toolbar_zoom = control.is_some_and(|control| control.toolbar_visible());
        let menu_zoom = control.is_some_and(|control| control.menu_visible());
        // A bottom/right dock's only panel cannot be closed through the
        // group (base keeps a dock's last group), but the skin handles that
        // close by removing the whole dock, so the item is offered.
        let closable = group.is_closable()
            || (self.is_dock_root_group(group, cx).is_some()
                && group.active_panel().is_some_and(|panel| panel.closable(cx)));
        let buttons = handle.and_then(|handle| handle.toolbar_buttons(window, cx));
        let panel = handle.map(|handle| handle.panel());

        h_flex()
            .gap_1()
            .occlude()
            .when_some(buttons, |this, buttons| {
                this.children(
                    buttons
                        .into_iter()
                        .map(|button| button.small().ghost().tab_stop(false)),
                )
            })
            .map(|this| {
                let value = if zoomed {
                    Some(("zoom-out", IconName::Minimize, t("Dock.Zoom Out")))
                } else if toolbar_zoom {
                    Some(("zoom-in", IconName::Maximize, t("Dock.Zoom In")))
                } else {
                    None
                };

                if let Some((id, icon, tooltip)) = value {
                    this.child(
                        Button::new(id)
                            .icon(icon)
                            .small()
                            .ghost()
                            .tab_stop(false)
                            .tooltip_with_action(tooltip, &ToggleZoom, None)
                            .selected(zoomed)
                            .on_click({
                                let group = group.clone();
                                move |_, window, cx| group.toggle_zoom(window, cx)
                            }),
                    )
                } else {
                    this
                }
            })
            .child(
                Button::new("menu")
                    .icon(IconName::Ellipsis)
                    .small()
                    .ghost()
                    .tab_stop(false)
                    .dropdown_menu(move |menu, window, cx| {
                        menu.when_some(panel.clone(), |menu, panel| {
                            panel.dropdown_menu(menu, window, cx)
                        })
                        .separator()
                        .menu_with_disabled(
                            if zoomed {
                                t("Dock.Zoom Out")
                            } else {
                                t("Dock.Zoom In")
                            },
                            Box::new(ToggleZoom),
                            !menu_zoom,
                        )
                        .when(closable, |menu| {
                            menu.separator().menu(t("Dock.Close"), Box::new(ClosePanel))
                        })
                    })
                    .anchor(Anchor::TopRight),
            )
    }

    /// One tab of the pill strip.
    ///
    /// While collapsed, tabs lose the active style and all interactions, and
    /// the strip becomes the way a closed bottom dock is opened again.
    #[allow(clippy::too_many_arguments)]
    fn render_tab(
        &self,
        group: &TabGroupContext,
        ix: usize,
        panel: Arc<dyn BasePanelView>,
        active: bool,
        is_bottom_dock: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Tab {
        let collapsed = group.is_collapsed();
        let droppable = group.is_droppable();
        let drag = self.tab_drag(group, ix, cx);
        let handle = PanelHandle::of(&panel);

        Tab::new(ix)
            .h_6()
            .px_3()
            .text_sm()
            .whitespace_nowrap()
            .flex()
            .items_center()
            .justify_center()
            .gap_1()
            .flex_shrink_0()
            .overflow_hidden()
            .rounded(cx.theme().radius)
            .text_color(cx.theme().foreground)
            .map(|this| match handle.and_then(|handle| handle.tab_name(cx)) {
                Some(tab_name) => this.child(tab_name),
                None => this.child(panel_title(&panel, window, cx)),
            })
            // Pill presentation: the selected tab is the filled pill, the
            // rest are transparent until hovered.
            .styles(|styles| {
                styles.selected(|style| {
                    style
                        .text_color(cx.theme().tab_active_foreground)
                        .bg(cx.theme().tab_active)
                })
            })
            .hover(|this| {
                if active {
                    this
                } else {
                    this.text_color(cx.theme().secondary_foreground)
                        .bg(cx.theme().secondary_hover)
                }
            })
            .selected(active)
            .on_click({
                let group = group.clone();
                let area = self.shared.area().clone();
                move |_, window, cx| {
                    group.select_tab(ix, window, cx);

                    // Clicking the strip of a collapsed bottom dock is how it
                    // is opened again.
                    if is_bottom_dock && collapsed {
                        _ = area.update(cx, |area, cx| {
                            area.toggle_dock(DockPlacement::Bottom, window, cx);
                        });
                    }
                }
            })
            .when(!collapsed, |this| {
                this.when_some(drag, |this, drag| {
                    this.on_drag(drag, {
                        let panel = panel.clone();
                        move |drag, offset, _, cx| {
                            cx.stop_propagation();
                            drag.set_drag_offset(offset);
                            drag.set_preview_size(DRAG_PREVIEW_SIZE);
                            cx.new(|_| DragPanelPreview {
                                panel: panel.clone(),
                            })
                        }
                    })
                })
                .when(droppable, |this| {
                    this.drag_over::<DragPanel>(|this, _, _, cx| {
                        this.rounded_l_none()
                            .border_l_2()
                            .border_r_0()
                            .border_color(cx.theme().drag_border)
                    })
                    .on_drop({
                        let group = group.clone();
                        move |drag: &DragPanel, window, cx| {
                            group.drop_panel(drag.clone(), Some(ix), true, window, cx);
                        }
                    })
                    .drag_over::<AnyDrag>(|this, _, _, cx| {
                        this.rounded_l_none()
                            .border_l_2()
                            .border_r_0()
                            .border_color(cx.theme().drag_border)
                    })
                    .on_drop({
                        let group = group.clone();
                        move |item: &AnyDrag, window, cx| {
                            group.drop_item(item.clone(), None, window, cx);
                        }
                    })
                })
            })
    }

    /// The strip after the last tab: a drop target for panels and host-owned
    /// drag items. Its left edge (right after the last tab) marks the start
    /// of the title-bar drag overlay.
    fn render_empty_space(
        &self,
        group: &TabGroupContext,
        tabs_count: usize,
        _cx: &mut App,
    ) -> AnyElement {
        let strip_bounds = self.title_bar_strip_bounds.clone();
        let shared = self.shared.clone();
        let droppable = group.is_droppable();

        let mut empty = div()
            .id("tab-bar-empty-space")
            .h_full()
            .flex_grow_1()
            .min_w_16()
            .on_prepaint(move |bounds, _, cx| {
                if strip_bounds.get() != Some(bounds) {
                    strip_bounds.set(Some(bounds));
                    _ = shared.area().update(cx, |_, cx| cx.notify());
                }
            });

        if droppable {
            empty = empty
                .drag_over::<DragPanel>(|this, _, _, cx| this.bg(cx.theme().tokens.drop_target))
                .on_drop({
                    let group = group.clone();
                    let node = group.node();
                    move |drag: &DragPanel, window, cx| {
                        // A panel dropped past its own last tab lands in the
                        // final slot; one from elsewhere is appended in the
                        // background.
                        let ix = (drag.source() == node).then(|| tabs_count - 1);
                        group.drop_panel(drag.clone(), ix, false, window, cx);
                    }
                })
                .drag_over::<AnyDrag>(|this, _, _, cx| this.bg(cx.theme().tokens.drop_target))
                .on_drop({
                    let group = group.clone();
                    move |item: &AnyDrag, window, cx| {
                        group.drop_item(item.clone(), None, window, cx);
                    }
                });
        }

        empty.into_any_element()
    }
}

impl TabGroupRenderer for SignedTabGroupSkin {
    fn frame(&self, group: &TabGroupContext, _: &mut Window, cx: &mut App) -> Stateful<Div> {
        let control = zoom_control(group, cx);
        // An emptied group — its last panel was dragged away — draws nothing,
        // so an emptied dock does not leave a bare tab bar behind.
        if group.panels().is_empty() {
            return div().id("tab-panel");
        }
        // Closing the only panel of a bottom/right dock would leave an empty
        // dock, which base refuses through the group. The skin removes the
        // whole dock instead — the vendored dock's close took its split
        // group away just the same.
        let dock_to_remove = (group.panels().len() <= 1)
            .then(|| self.is_dock_root_group(group, cx))
            .flatten();
        let shared = self.shared.clone();

        // `v_flex`, not `div`: gpui's default display is Block, and in block
        // layout a child's `flex_grow` is ignored — the content region below
        // the tab bar would resolve to zero height.
        v_flex()
            .id("tab-panel")
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().tokens.background)
            // A collapsed group is a strip of tabs with no content, and the
            // actions act on content.
            .when(!group.is_collapsed(), |this| {
                this.on_action({
                    let group = group.clone();
                    move |_: &ToggleZoom, window, cx| {
                        // The affordance decides the control, so a panel that
                        // offers none is not zoomed *in* by the keybinding
                        // either. Zooming out is never refused: a panel that
                        // stopped offering the control while zoomed would
                        // otherwise strand the user with no way back.
                        if !group.is_zoomed() && control.is_none() {
                            return;
                        }
                        group.toggle_zoom(window, cx);
                    }
                })
                .on_action({
                    let group = group.clone();
                    let shared = shared.clone();
                    move |_: &ClosePanel, window, cx| {
                        let Some(panel) = group.active_panel() else {
                            return;
                        };
                        if !panel.closable(cx) {
                            return;
                        }
                        let panel = panel.panel_id(cx);
                        match dock_to_remove {
                            Some(placement) => {
                                _ = shared.area().update(cx, |area, cx| {
                                    area.remove_dock(placement, window, cx);
                                });
                            }
                            None => group.close(panel, window, cx),
                        }
                    }
                })
            })
    }

    fn content_frame(&self, group: &TabGroupContext, _: &mut Window, _: &mut App) -> Stateful<Div> {
        v_flex()
            .id("active-panel")
            // A collapsed group draws its tab strip and nothing else, so the
            // content region must not claim any space.
            .when(!group.is_collapsed(), |this| this.flex_1())
    }

    fn render_tab_bar(
        &self,
        group: &TabGroupContext,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        // An emptied group draws no tab bar; the app prunes the emptied
        // bottom/right dock a moment later.
        if group.panels().is_empty() {
            return Empty.into_any_element();
        }

        // The sidebar group draws no chrome at all, like the vendored dock's
        // bare `DockItem::Panel`.
        if self.is_plain_sidebar_group(group, cx) {
            return Empty.into_any_element();
        }

        let collapsed = group.is_collapsed();
        let active_ix = group.active_ix();
        let tabs_count = group.panels().len();

        let left_dock_button = self.dock_toggle_button(DockPlacement::Left, group, cx);
        let bottom_dock_button = self.dock_toggle_button(DockPlacement::Bottom, group, cx);
        let right_dock_button = self.dock_toggle_button(DockPlacement::Right, group, cx);
        let is_bottom_dock = bottom_dock_button.is_some();

        // macOS: the traffic lights overlay the window's top-left corner. Only
        // the group whose tab bar actually sits under them must reserve the
        // space: the left dock (sidebar) normally clears them, and when it is
        // closed or absent it is the center's left-most, top-most tab group
        // that is in the corner. A bottom or right dock is never there, and
        // neither is the right panel of a center split.
        let needs_traffic_light_padding = cfg!(target_os = "macos")
            && self.shared.area().upgrade().is_some_and(|area| {
                let area = area.read(cx);
                !area.is_dock_open(DockPlacement::Left)
                    && area
                        .layout(DockPlacement::Center)
                        .and_then(|tree| left_top_group(tree.root()))
                        == Some(group.node())
            });

        // Bring a newly displayed tab into view. The group owns selection
        // now, so the skin notices the change rather than being told about it.
        let displayed = group.active_panel().map(|panel| panel.panel_id(cx));
        let visible: Vec<usize> = group
            .panels()
            .iter()
            .enumerate()
            .filter(|(_, panel)| panel.visible(cx))
            .map(|(ix, _)| ix)
            .collect();
        if self.last_active_ix.replace(Some(active_ix)) != Some(active_ix)
            && let Some(visible_ix) = visible.iter().position(|ix| *ix == active_ix)
        {
            self.scroll_handle.scroll_to_item(visible_ix);
        }

        // The tab strip lays out its scrollable content at content width, so
        // the area after the last tab only spans `min_w_16` — the rest of the
        // tab bar has no element at all. Cover that dead zone with a
        // measured overlay so the whole non-interactive area can drag the
        // window. Its span is [last tab's right edge, suffix's left edge].
        let drag_overlay = match (
            self.title_bar_bounds.get(),
            self.title_bar_strip_bounds.get(),
            self.title_bar_suffix_bounds.get(),
        ) {
            (Some(title), Some(strip), Some(suffix)) => {
                let left = strip.left() - title.left();
                let right = title.right() - suffix.left();
                (left + right < title.size.width).then(|| {
                    title_bar_drag_handlers(
                        div()
                            .id(("title-bar-drag", group.node().as_u64()))
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(left)
                            .right(right),
                        window,
                        cx,
                    )
                })
            }
            _ => None,
        };

        let tabs: Vec<_> = group
            .panels()
            .iter()
            .enumerate()
            .filter_map(|(ix, panel)| {
                let mut active = displayed == Some(panel.panel_id(cx));
                if !panel.visible(cx) {
                    return None;
                }
                // A collapsed group shows no tab as active: the strip is a
                // way back in, not a selection.
                if collapsed {
                    active = false;
                }
                Some(self.render_tab(group, ix, panel.clone(), active, is_bottom_dock, window, cx))
            })
            .collect();

        let empty_space = self.render_empty_space(group, tabs_count, cx);
        let title_bar_bounds = self.title_bar_bounds.clone();
        let title_bar_suffix_bounds = self.title_bar_suffix_bounds.clone();
        let shared = self.shared.clone();
        let suffix_shared = self.shared.clone();

        div()
            .flex()
            .flex_row()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .on_prepaint(move |bounds, _, cx| {
                        if title_bar_bounds.get() != Some(bounds) {
                            title_bar_bounds.set(Some(bounds));
                            _ = shared.area().update(cx, |_, cx| cx.notify());
                        }
                    })
                    .child(
                        Tabs::new("tab-bar")
                            .px(px(-1.))
                            .h(TAB_BAR_HEIGHT)
                            .flex()
                            .items_center()
                            .text_color(cx.theme().tab_foreground)
                            .child(
                                h_flex()
                                    .items_center()
                                    .top_0()
                                    // Right -1 for avoid border overlap with the first tab
                                    .right(-px(1.))
                                    .h_full()
                                    .gap_2()
                                    .px_2()
                                    .when(needs_traffic_light_padding, |this| this.pl(px(80.)))
                                    .children(left_dock_button)
                                    .children(bottom_dock_button)
                                    .child(self.render_prev_next_tab_buttons(group, cx)),
                            )
                            .child(
                                h_flex().id("tabs").flex_1().overflow_x_hidden().child(
                                    h_flex()
                                        .id("tabs-inner")
                                        .relative()
                                        .gap(px(4.))
                                        .overflow_x_scroll()
                                        .lock_scroll_axis()
                                        .track_scroll(&self.scroll_handle)
                                        .children(tabs)
                                        .when(!collapsed, |this| this.child(empty_space)),
                                ),
                            )
                            .when(!collapsed, |this| {
                                this.child(
                                    h_flex()
                                        .items_center()
                                        .top_0()
                                        .right_0()
                                        .h_full()
                                        .px_2()
                                        .gap_1()
                                        .on_prepaint(move |bounds, _, cx| {
                                            if title_bar_suffix_bounds.get() != Some(bounds) {
                                                title_bar_suffix_bounds.set(Some(bounds));
                                                _ = suffix_shared
                                                    .area()
                                                    .update(cx, |_, cx| cx.notify());
                                            }
                                        })
                                        .children(
                                            group
                                                .active_panel()
                                                .and_then(PanelHandle::of)
                                                .and_then(|handle| handle.title_suffix(window, cx)),
                                        )
                                        .child(self.render_toolbar(group, window, cx))
                                        .children(right_dock_button),
                                )
                            }),
                    )
                    .when_some(drag_overlay, |this, overlay| this.child(overlay)),
            )
            .child(window_controls::window_controls(window, cx))
            .into_any_element()
    }

    fn render_active_panel(
        &self,
        panel: AnyView,
        group: &TabGroupContext,
        _: &mut Window,
        _: &mut App,
    ) -> AnyElement {
        if group.is_collapsed() {
            return Empty.into_any_element();
        }

        div()
            .id("tab-content")
            .overflow_y_scroll()
            .overflow_x_hidden()
            .flex_1()
            .child(panel.cached(StyleRefinement::default().absolute().size_full()))
            .into_any_element()
    }

    fn render_drop_indicator(
        &self,
        indicator: DropIndicator,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let (from, to) = (indicator.from(), indicator.to());
        // The placeholder animates from wherever it was to where the drop
        // would land, so its own element is positioned at the destination and
        // the animation only has to walk the difference back to zero.
        let offset = from.origin() - to.origin();

        Some(
            div()
                .absolute()
                .left(to.origin().x)
                .top(to.origin().y)
                .w(to.size().width)
                .h(to.size().height)
                .child(
                    div()
                        .absolute()
                        .bg(cx.theme().tokens.drop_target)
                        .with_animation(
                            gpui::ElementId::NamedInteger(
                                "drop-placeholder".into(),
                                indicator.epoch(),
                            ),
                            Animation::new(Duration::from_millis(150)).with_easing(ease_out_cubic),
                            move |this, delta| {
                                let origin = offset.lerp(&Point::default(), delta);
                                let width = from.size().width.lerp(&to.size().width, delta);
                                let height = from.size().height.lerp(&to.size().height, delta);
                                this.left(origin.x).top(origin.y).w(width).h(height)
                            },
                        ),
                )
                .into_any_element(),
        )
    }
}
