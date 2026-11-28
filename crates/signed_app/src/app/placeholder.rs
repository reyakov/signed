use gpui::{Context, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window};
use ui::empty_state::EmptyState;
use ui::layout::v_flex;

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
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .child(EmptyState::new(self.title.clone()))
    }
}
