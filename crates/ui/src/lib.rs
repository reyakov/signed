//! Signed's native GPUI layer: theme, icons, components, and layout.

pub mod components;
pub mod icons;
pub mod layout;
pub mod theme;

pub use components::*;
use gpui::App;
pub use icons::{Assets, Icon, IconRole};

/// Initializes the theme and every key binding the UI layer owns.
pub fn init(cx: &mut App) {
    theme::Theme::init(cx);
    components::input::bind_input_keys(cx, None);
    components::combobox::bind_combobox_keys(cx);
    components::command_palette::bind_command_palette_keys(cx);
    components::code::bind_code_view_keys(cx);
    components::markdown::bind_text_view_keys(cx);
}
