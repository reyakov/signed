use gpui::App;
use gpui_native_kit::theme::{ThemeVariant, follow_system, set_mode_override};
use settings::{AppearanceMode, Settings};

/// Applies the persisted light/dark/system preference to the kit theme.
pub fn apply_appearance(settings: &Settings, cx: &mut App) {
    apply_appearance_mode(settings.appearance, cx);
}

/// Applies one appearance mode to the kit theme.
pub fn apply_appearance_mode(mode: AppearanceMode, cx: &mut App) {
    match mode {
        AppearanceMode::System => follow_system(cx),
        AppearanceMode::Light => set_mode_override(Some(ThemeVariant::Light), cx),
        AppearanceMode::Dark => set_mode_override(Some(ThemeVariant::Dark), cx),
    }
}
