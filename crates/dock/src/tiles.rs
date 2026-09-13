use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Div, DragMoveEvent, Empty, InteractiveElement as _,
    IntoElement, MouseButton, MouseDownEvent, ParentElement as _, Pixels, Render, ScrollHandle,
    Size, Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use gpui_base::dock::{
    DRAG_BAR_HEIGHT, HANDLE_SIZE, NodeId, ResizeSide, TileContext, TilesRenderer,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::dock::PanelHandle;
use gpui_component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, h_flex, v_flex,
};

use crate::dock_area::SkinShared;
use crate::t;
use crate::tab_panel::panel_title;

/// How far a resize handle sticks out past the tile's edge.
const HANDLE_OFFSET: Pixels = px(-4.);

/// The payload a tile drag carries, so one canvas ignores another's drags.
#[derive(Clone)]
struct DragMoving(NodeId);

impl Render for DragMoving {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// The payload a tile resize carries, for the same reason.
#[derive(Clone)]
struct DragResizing(NodeId);

impl Render for DragResizing {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// One tiles canvas's appearance.
/// Built once per container, so its scroll position belongs to the canvas it scrolls.
pub(crate) struct SignedTilesSkin {
    shared: Rc<SkinShared>,
    scroll_handle: ScrollHandle,
}

impl SignedTilesSkin {
    pub(crate) fn new(shared: Rc<SkinShared>) -> Self {
        Self {
            shared,
            scroll_handle: ScrollHandle::default(),
        }
    }

    fn resize_handle(
        &self,
        tile: &TileContext,
        id: &'static str,
        side: ResizeSide,
        build: impl FnOnce(Stateful<Div>) -> Stateful<Div>,
    ) -> Stateful<Div> {
        let node = tile.node();

        build(div().id(id).absolute())
            .on_mouse_down(MouseButton::Left, {
                let tile = tile.clone();
                move |event: &MouseDownEvent, window, cx| {
                    tile.begin_resize(side, event.position, window, cx);
                    cx.stop_propagation();
                }
            })
            .on_drag(DragResizing(node), |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            })
            .on_drag_move({
                let tile = tile.clone();
                move |event: &DragMoveEvent<DragResizing>, window, cx| {
                    if event.drag(cx).0 != node {
                        return;
                    }
                    tile.resize_to(event.event.position, window, cx);
                }
            })
    }

    /// The trailing controls of a tile's title bar, zoom, close and the ellipsis menu.
    /// They use click handlers, the zoom and close actions target a focused tab group.
    fn render_tile_controls(
        &self,
        tile: &TileContext,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let handle = PanelHandle::of(tile.panel());
        let control = handle.and_then(|handle| handle.zoom_control(cx));
        let zoomed = tile.is_zoomed();
        let toolbar_zoom =
            tile.is_zoomable() && control.is_some_and(|control| control.toolbar_visible());
        let menu_zoom = tile.is_zoomable() && control.is_some_and(|control| control.menu_visible());
        let closable = tile.is_closable();
        let buttons = handle.and_then(|handle| handle.toolbar_buttons(window, cx));
        let panel = handle.map(|handle| handle.panel());

        h_flex()
            .gap_1()
            .flex_shrink_0()
            .occlude()
            .when_some(buttons, |this, buttons| {
                this.children(
                    buttons
                        .into_iter()
                        .map(|button| button.xsmall().ghost().tab_stop(false)),
                )
            })
            .when_some(
                match (zoomed, toolbar_zoom) {
                    (true, _) => Some(("zoom-out", IconName::Minimize, t("Dock.Zoom Out"))),
                    (false, true) => Some(("zoom-in", IconName::Maximize, t("Dock.Zoom In"))),
                    (false, false) => None,
                },
                |this, (id, icon, tooltip)| {
                    this.child(
                        Button::new(id)
                            .icon(icon)
                            .xsmall()
                            .ghost()
                            .tab_stop(false)
                            .tooltip(tooltip)
                            .selected(zoomed)
                            .on_click({
                                let tile = tile.clone();
                                move |_, window, cx| tile.toggle_zoom(window, cx)
                            }),
                    )
                },
            )
            .child(
                Button::new("menu")
                    .icon(IconName::Ellipsis)
                    .xsmall()
                    .ghost()
                    .tab_stop(false)
                    .dropdown_menu({
                        let tile = tile.clone();
                        move |menu, window, cx| {
                            menu.when_some(panel.clone(), |menu, panel| {
                                panel.dropdown_menu(menu, window, cx)
                            })
                            .separator()
                            .item(
                                PopupMenuItem::new(match zoomed {
                                    true => t("Dock.Zoom Out"),
                                    false => t("Dock.Zoom In"),
                                })
                                .disabled(!menu_zoom && !zoomed)
                                .on_click({
                                    let tile = tile.clone();
                                    move |_, window, cx| tile.toggle_zoom(window, cx)
                                }),
                            )
                            .when(closable, |menu| {
                                menu.separator()
                                    .item(PopupMenuItem::new(t("Dock.Close")).on_click({
                                        let tile = tile.clone();
                                        move |_, window, cx| tile.close(window, cx)
                                    }))
                            })
                        }
                    })
                    .anchor(gpui::Anchor::TopRight),
            )
    }
}

impl TilesRenderer for SignedTilesSkin {
    fn frame(&self, _: &mut Window, cx: &mut App) -> Stateful<Div> {
        div()
            .id("tiles")
            .relative()
            .size_full()
            .bg(cx.theme().tokens.tiles)
            .track_scroll(&self.scroll_handle)
            .overflow_scroll()
    }

    fn tile_frame(&self, tile: &TileContext, _: &mut Window, cx: &mut App) -> Stateful<Div> {
        v_flex()
            .id(("tile", tile.panel_id().as_u64()))
            .occlude()
            .overflow_hidden()
            .bg(cx.theme().tokens.background)
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().tile_radius)
            // Room for the title bar, which overlays the top padding.
            // Base draws the panel as a plain child, this keeps them apart.
            .pt(DRAG_BAR_HEIGHT)
            // Base stores no bounds on a zoomed tile, the skin decides how it fills the dock.
            .when(tile.is_zoomed(), |this| this.size_full())
            .on_mouse_down(MouseButton::Left, {
                let tile = tile.clone();
                move |_, window, cx| tile.bring_to_front(window, cx)
            })
            // A gesture can end anywhere, so both mouse-up hooks run.
            // Each is a no-op unless this tile is the one that moved.
            .on_mouse_up(MouseButton::Left, {
                let tile = tile.clone();
                move |_, window, cx| {
                    tile.end_move(window, cx);
                    tile.end_resize(window, cx);
                }
            })
            .on_mouse_up_out(MouseButton::Left, {
                let tile = tile.clone();
                move |_, window, cx| {
                    tile.end_move(window, cx);
                    tile.end_resize(window, cx);
                }
            })
    }

    fn render_drag_bar(&self, tile: &TileContext, window: &mut Window, cx: &mut App) -> AnyElement {
        let node = tile.node();
        let handle = PanelHandle::of(tile.panel());
        let title_style = handle.and_then(|handle| handle.title_style(cx));

        h_flex()
            .id("drag-bar")
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h(DRAG_BAR_HEIGHT)
            .items_center()
            .gap_1()
            .pl_3()
            .pr_2()
            .when_some(title_style, |this, style| {
                this.bg(style.background).text_color(style.foreground)
            })
            .child(
                div()
                    .flex_1()
                    .min_w_16()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(panel_title(tile.panel(), window, cx)),
            )
            .children(handle.and_then(|handle| handle.title_suffix(window, cx)))
            .child(self.render_tile_controls(tile, window, cx))
            // A zoomed tile is not at its stored bounds, so moving it would mean nothing.
            .when(!tile.is_zoomed(), |this| {
                this.cursor_grab()
                    .on_mouse_down(MouseButton::Left, {
                        let tile = tile.clone();
                        move |event: &MouseDownEvent, window, cx| {
                            tile.begin_move(event.position, window, cx);
                        }
                    })
                    .on_drag(DragMoving(node), |drag, _, _, cx| {
                        cx.stop_propagation();
                        cx.new(|_| drag.clone())
                    })
                    .on_drag_move({
                        let tile = tile.clone();
                        move |event: &DragMoveEvent<DragMoving>, window, cx| {
                            if event.drag(cx).0 != node {
                                return;
                            }
                            tile.move_to(event.event.position, window, cx);
                        }
                    })
            })
            .into_any_element()
    }

    fn render_resize_handles(
        &self,
        tile: &TileContext,
        _: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let bounds = tile.bounds();

        // A passive full-tile box, so handles sit against the tile, not its flow neighbours.
        // It registers no interaction, so it does not shadow the panel underneath.
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(
                self.resize_handle(tile, "left-resize-handle", ResizeSide::Left, |this| {
                    this.cursor_ew_resize()
                        .top_0()
                        .left(HANDLE_OFFSET)
                        .w(HANDLE_SIZE)
                        .h(bounds.size.height)
                }),
            )
            .child(
                self.resize_handle(tile, "right-resize-handle", ResizeSide::Right, |this| {
                    this.cursor_ew_resize()
                        .top_0()
                        .right(HANDLE_OFFSET)
                        .w(HANDLE_SIZE)
                        .h(bounds.size.height)
                }),
            )
            .child(
                self.resize_handle(tile, "top-resize-handle", ResizeSide::Top, |this| {
                    this.cursor_ns_resize()
                        .left_0()
                        .top(HANDLE_OFFSET)
                        .w(bounds.size.width)
                        .h(HANDLE_SIZE)
                }),
            )
            .child(
                self.resize_handle(tile, "bottom-resize-handle", ResizeSide::Bottom, |this| {
                    this.cursor_ns_resize()
                        .left_0()
                        .bottom(HANDLE_OFFSET)
                        .w(bounds.size.width)
                        .h(HANDLE_SIZE)
                }),
            )
            .child(
                Icon::new(IconName::ResizeCorner)
                    .size_3()
                    .absolute()
                    .right(px(1.))
                    .bottom(px(1.))
                    .text_color(cx.theme().muted_foreground.opacity(0.5)),
            )
            .child(self.resize_handle(
                tile,
                "corner-resize-handle",
                ResizeSide::BottomRight,
                |this| {
                    this.cursor_nwse_resize()
                        .right(HANDLE_OFFSET)
                        .bottom(HANDLE_OFFSET)
                        .size_3()
                },
            ))
            .into_any_element()
    }

    /// Gives the tile's panel `size_full`, base draws it as a plain child otherwise.
    fn panel_frame(&self, tile: &TileContext, _: &mut Window, _: &mut App) -> Stateful<Div> {
        h_flex()
            .id(("tile-panel", tile.panel_id().as_u64()))
            .overflow_hidden()
            .size_full()
    }

    /// The canvas scrollbar, as an overlay.
    /// Placed inside the frame it would end up underneath every tile.
    fn render_overlay(
        &self,
        content: Size<Pixels>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<AnyElement> {
        Some(
            Scrollbar::new(&self.scroll_handle)
                .scroll_size(content)
                .when_some(self.shared.tiles_scrollbar_mode(), |this, mode| {
                    this.mode(mode)
                })
                .into_any_element(),
        )
    }

    fn grid_size(&self, cx: &App) -> Pixels {
        cx.theme().tile_grid_size
    }
}
