use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Window, div,
};
use gpui_native_kit::empty_state::EmptyState;
use gpui_native_kit::layout::dock::DockState;
use gpui_native_kit::layout::shell::{LayoutState, Shell};
use gpui_native_kit::theme::{ActiveTheme as _, ThemeVariant, follow_system, set_mode_override};
use settings::{AppearanceMode, Settings};

use crate::dialogs::DialogHost;

pub struct AppRoot {
    layout: Entity<LayoutState>,
    dock: Entity<DockState>,
    dialogs: Entity<DialogHost>,
}

impl AppRoot {
    pub fn new(_window: &mut Window, cx: &mut App) -> Self {
        let layout = LayoutState::new(cx);
        let dock = DockState::new(cx);
        let dialogs = cx.new(DialogHost::new);
        DialogHost::set_global(dialogs.clone(), cx);

        Self {
            layout,
            dock,
            dialogs,
        }
    }
}

impl Render for AppRoot {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .size_full()
            .font_family(".SystemUIFont")
            .text_color(theme.colors.foreground)
            .child(
                Shell::new("signed-shell", self.layout.clone())
                    .center(EmptyState::new("Signed").hint("UI migration in progress")),
            )
            .child(self.dialogs.clone())
    }
}

pub fn apply_appearance(settings: &Settings, cx: &mut App) {
    match settings.appearance {
        AppearanceMode::System => follow_system(cx),
        AppearanceMode::Light => set_mode_override(Some(ThemeVariant::Light), cx),
        AppearanceMode::Dark => set_mode_override(Some(ThemeVariant::Dark), cx),
    }
}
