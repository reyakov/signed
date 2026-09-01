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

use gpui::{Pixels, px};

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
