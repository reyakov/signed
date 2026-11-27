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
    components::input::InputState::bind_keys(cx);
    components::combobox::Combobox::<()>::bind_keys(cx);
    components::command_palette::CommandPaletteState::bind_keys(cx);
    components::code::CodeViewState::bind_keys(cx);
    components::markdown::MarkdownViewState::bind_keys(cx);
    components::menu::Menu::bind_keys(cx);
    components::dialog::Dialog::bind_keys(cx);
}
