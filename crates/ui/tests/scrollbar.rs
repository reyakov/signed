//! A hidden auto-hide scrollbar repaints its owner as soon as the pointer
//! reaches its track.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Context, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _, Render,
    ScrollHandle, StatefulInteractiveElement as _, Styled as _, TestAppContext, VisualTestContext,
    Window, div, point, px, size,
};
use ui::scrollbar::{Scrollbar, ScrollbarVisibility};
use ui::theme::Theme;

const WINDOW_WIDTH: f32 = 800.0;
const WINDOW_HEIGHT: f32 = 600.0;
const ROW_HEIGHT: f32 = 40.0;
const ROWS: usize = 50;

/// A scrolled surface with an auto-hide scrollbar overlaid on its right edge.
struct ScrollbarHarness {
    handle: ScrollHandle,
    renders: Rc<Cell<usize>>,
}

impl Render for ScrollbarHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div()
            .relative()
            .size_full()
            .child(
                div()
                    .id("log")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.handle)
                    .children(
                        (0..ROWS).map(|row| {
                            div().w_full().h(px(ROW_HEIGHT)).child(format!("row {row}"))
                        }),
                    ),
            )
            .child(
                Scrollbar::vertical("log-bar", self.handle.clone())
                    .visibility(ScrollbarVisibility::Autohide),
            )
    }
}

#[gpui::test]
fn hovering_a_hidden_scrollbar_repaints_its_owner(cx: &mut TestAppContext) {
    cx.update(|cx| Theme::init_with(cx, Theme::fallback_source()));
    let renders = Rc::new(Cell::new(0));
    let harness = {
        let renders = renders.clone();
        move |_: &mut Window, _: &mut Context<ScrollbarHarness>| ScrollbarHarness {
            handle: ScrollHandle::new(),
            renders,
        }
    };
    let window = cx.open_window(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), harness);
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    renders.set(0);

    // A scrollbar that has never been touched is hidden; reaching its track must
    // repaint the owner so the thumb can appear.
    cx.simulate_mouse_move(
        point(px(WINDOW_WIDTH - 2.), px(WINDOW_HEIGHT / 2.0)),
        None,
        Modifiers::default(),
    );
    cx.update(|_, _| {});
    cx.run_until_parked();

    assert!(
        renders.get() > 0,
        "reaching the track of a hidden scrollbar repaints its owner"
    );
}
