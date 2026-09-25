use std::sync::Arc;

use gpui::{Context, Pixels, Window, px};
use gpui_base::dock::PanelView;

mod dock_area;
mod invalid_panel;
mod tab_panel;
mod window_controls;

pub use dock_area::SignedDockSkin;
pub use gpui_component::dock::{
    BasePanel, DockArea, DockEvent, DockLayout, DockPlacement, Panel, PanelEvent, panel_handle,
};

/// Add an already-wrapped panel handle to the center of `area`.
///
/// Every panel entry point opens its panel there.
pub fn add_center_panel(
    area: &mut DockArea,
    panel: Arc<dyn PanelView>,
    window: &mut Window,
    cx: &mut Context<DockArea>,
) {
    area.add_panel_view(panel, DockPlacement::Center, None, window, cx);
}

/// Add an already-wrapped panel handle to the bottom dock of `area`.
///
/// Used for sub-views that hang under the center, such as the inbox's Unread
/// and Archived lists.
pub fn add_bottom_panel(
    area: &mut DockArea,
    panel: Arc<dyn PanelView>,
    window: &mut Window,
    cx: &mut Context<DockArea>,
) {
    area.add_panel_view(panel, DockPlacement::Bottom, None, window, cx);
}

/// The fixed height of the tab bar, which doubles as the window title bar.
pub const TAB_BAR_HEIGHT: Pixels = px(44.);

/// i18n shim resolving `Dock.*` keys to English, so the crate has no i18n dependency.
pub(crate) fn t(key: &'static str) -> &'static str {
    match key {
        "Dock.Close" => "Close",
        "Dock.Zoom In" => "Zoom In",
        "Dock.Zoom Out" => "Zoom Out",
        "Dock.Collapse" => "Collapse",
        "Dock.Expand" => "Expand",
        _ => key,
    }
}
