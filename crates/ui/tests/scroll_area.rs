//! A covering overlay swallows scrollbar presses instead of dragging the thumb
//! behind it.

use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, InteractiveElement as _, IntoElement, Modifiers, MouseButton, ParentElement as _,
    Render, ScrollHandle, Styled as _, TestAppContext, VisualTestContext, Window, div, point, px,
    size,
};
use ui::scroll_area::ScrollArea;
use ui::theme::Theme;

const WINDOW_WIDTH: f32 = 800.0;
const WINDOW_HEIGHT: f32 = 600.0;
const VIEWPORT_HEIGHT: f32 = 100.0;
const ROW_HEIGHT: f32 = 40.0;
const ROWS: usize = 50;
/// The track's thickness, matching the theme's scrollbar width.
const TRACK_WIDTH: f32 = 8.0;

/// A scroll area over a full-window cover the test toggles.
struct ScrollHarness {
    handle: ScrollHandle,
    covered: bool,
}

impl Render for ScrollHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .child(
                ScrollArea::new("scroll-test")
                    .handle(self.handle.clone())
                    .w_full()
                    .h(px(VIEWPORT_HEIGHT))
                    .children(
                        (0..ROWS).map(|row| {
                            div().w_full().h(px(ROW_HEIGHT)).child(format!("row {row}"))
                        }),
                    ),
            )
            .when(self.covered, |this| {
                this.child(div().absolute().inset_0().occlude())
            })
    }
}

/// A press and release in the middle of the scrollbar's track.
fn press_track(cx: &mut VisualTestContext) {
    let track = point(
        px(WINDOW_WIDTH - TRACK_WIDTH / 2.0),
        px(VIEWPORT_HEIGHT * 0.8),
    );
    cx.simulate_mouse_down(track, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_up(track, MouseButton::Left, Modifiers::default());
}

#[gpui::test]
fn a_covering_overlay_blocks_scrollbar_drags(cx: &mut TestAppContext) {
    cx.update(Theme::init);
    let window = cx.open_window(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), |_, _| {
        ScrollHarness {
            handle: ScrollHandle::new(),
            covered: true,
        }
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();

    let harness = window.root(&mut cx).expect("harness view");
    let handle = cx.update(|_, cx| harness.read(cx).handle.clone());

    press_track(&mut cx);
    assert_eq!(
        handle.offset().y,
        px(0.),
        "the cover swallows the press on the track behind it"
    );

    cx.update(|_, cx| {
        harness.update(cx, |harness, cx| {
            harness.covered = false;
            cx.notify();
        });
    });
    cx.run_until_parked();

    press_track(&mut cx);
    assert!(
        handle.offset().y < px(0.),
        "an uncovered press on the track scrolls the content"
    );
}
