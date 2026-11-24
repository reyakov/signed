use gpui::{
    Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div,
};
use ui::empty_state::EmptyState;

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

impl Render for PlaceholderView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(EmptyState::new(self.title.clone()))
    }
}
