//! Ending a composition repaints the field, so its underline disappears.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    EntityInputHandler, Modifiers, Styled as _, TestAppContext, VisualTestContext, px, size,
};
use ui::input::InputState;
use ui::theme::Theme;

struct EmptyView;

impl gpui::Render for EmptyView {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::div().size_full()
    }
}

#[gpui::test]
fn unmarking_a_composition_notifies_the_field(cx: &mut TestAppContext) {
    cx.update(|cx| Theme::init_with(cx, Theme::fallback_source()));
    let window = cx.open_window(size(px(400.), px(100.)), |_, _| EmptyView);
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();

    let state = cx.update(|_, cx| InputState::new(cx));
    let notifies = Rc::new(Cell::new(0));
    let counted = notifies.clone();
    let _subscription =
        cx.update(|_, cx| cx.observe(&state, move |_, _| counted.set(counted.get() + 1)));

    cx.update(|window, cx| {
        state.update(cx, |state, cx| {
            EntityInputHandler::replace_and_mark_text_in_range(
                state,
                None,
                "ab",
                Some(0..2),
                window,
                cx,
            );
        });
    });
    cx.run_until_parked();
    let marked = notifies.get();

    cx.update(|window, cx| {
        state.update(cx, |state, cx| {
            EntityInputHandler::unmark_text(state, window, cx);
        });
    });
    cx.run_until_parked();

    assert!(
        notifies.get() > marked,
        "ending the composition repaints the field"
    );
    let _ = Modifiers::default();
}
