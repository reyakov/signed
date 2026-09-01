//! The Signed dock skin.
//!
//! The dock engine lives upstream (`gpui_base::dock` owns the layout tree,
//! drags, zoom and persistence); this crate is the appearance the app used
//! to vendor from gpui-component — a 44px tab bar that doubles as the
//! window title bar, with pill tabs, window controls, title-bar dragging
//! and previous/next tab buttons.
//!
//! Everything `gpui_component::dock` exports is re-exported here, so the app
//! keeps importing the dock from a single place.

use gpui::{
    App, Div, InteractiveElement as _, MouseButton, Pixels, Stateful,
    StatefulInteractiveElement as _, Window, WindowControlArea, px,
};

mod dock_area;
mod invalid_panel;
mod tab_panel;
mod tiles;
mod window_controls;

pub use dock_area::SignedDockSkin;
pub use gpui_component::dock::{
    AnyDrag, BasePanel, BasePanelView, ClosePanel, DockArea, DockAreaState, DockContext, DockEvent,
    DockLayout, DockPlacement, DockState, DragPanel, DropIndicator, DropPlaceholderBounds,
    DropTarget, Panel, PanelControl, PanelEvent, PanelHandle, PanelInfo, PanelState, PanelStyle,
    PanelView, TitleStyle, ToggleZoom, panel_handle, register_panel,
};

/// The fixed height of the tab bar, which doubles as the window title bar.
pub const TAB_BAR_HEIGHT: Pixels = px(44.);

/// Minimal i18n shim replacing gpui-component's `rust_i18n::t!()`, keeping the
/// same `Dock.*` keys resolved to English so the crate has no i18n dependency.
pub(crate) fn t(key: &'static str) -> &'static str {
    match key {
        "Dock.Unnamed" => "Unnamed",
        "Dock.Close" => "Close",
        "Dock.Zoom In" => "Zoom In",
        "Dock.Zoom Out" => "Zoom Out",
        "Dock.Collapse" => "Collapse",
        "Dock.Expand" => "Expand",
        _ => key,
    }
}

/// State used to move the window when the title bar area is dragged.
struct WindowDragState {
    should_move: bool,
}

/// Make an element behave like a window title bar: dragging it moves the
/// window, and double-clicking zooms the window (or performs the platform's
/// default title-bar double-click action on macOS).
///
/// Only the bar's non-interactive areas should get this — tabs are draggable
/// (to reorder panels) and must not move the window.
pub fn title_bar_drag_handlers(
    this: Stateful<Div>,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let state = window.use_state(cx, |_, _| WindowDragState { should_move: false });

    this.window_control_area(WindowControlArea::Drag)
        .on_mouse_down_out(window.listener_for(&state, |state, _, _, _| {
            state.should_move = false;
        }))
        .on_mouse_down(
            MouseButton::Left,
            window.listener_for(&state, |state, _, _, _| {
                state.should_move = true;
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            window.listener_for(&state, |state, _, _, _| {
                state.should_move = false;
            }),
        )
        .on_mouse_move(window.listener_for(&state, |state, _, window, _| {
            if state.should_move {
                state.should_move = false;
                window.start_window_move();
            }
        }))
        .on_click(|event, window, _| {
            if event.click_count() == 2 {
                if cfg!(target_os = "macos") {
                    window.titlebar_double_click();
                } else {
                    window.zoom_window();
                }
            }
        })
}
