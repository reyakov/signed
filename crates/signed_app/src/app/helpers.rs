use gpui::App;
use settings::AppearanceMode;
use ui::theme::{ThemeVariant, follow_system, set_mode_override};

/// Applies one appearance mode to the kit theme.
pub fn apply_appearance_mode(mode: AppearanceMode, cx: &mut App) {
    match mode {
        AppearanceMode::System => follow_system(cx),
        AppearanceMode::Light => set_mode_override(Some(ThemeVariant::Light), cx),
        AppearanceMode::Dark => set_mode_override(Some(ThemeVariant::Dark), cx),
    }
}
