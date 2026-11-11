use gpui::{
    App, Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div,
};
use gpui_native_kit::empty_state::EmptyState;
use gpui_native_kit::layout::dock::Panel;

/// A tab that stands in for a screen that is not ported yet.
pub struct PlaceholderView {
    title: SharedString,
}

impl PlaceholderView {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
        }
    }
}

impl Panel for PlaceholderView {
    fn title(&self, _cx: &App) -> Option<SharedString> {
        Some(self.title.clone())
    }
}

impl Render for PlaceholderView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(EmptyState::new(self.title.clone()))
    }
}
