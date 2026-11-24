//! A popover takes focus on open and gives it back to what held it on close.

use gpui::{
    Context, FocusHandle, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _,
    Render, Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
};
use ui::common::v_flex;
use ui::popover::Popover;
use ui::theme::Theme;

const WINDOW_WIDTH: f32 = 800.0;
const WINDOW_HEIGHT: f32 = 600.0;

/// A popover over a focusable trigger, with its open state held by the view.
struct PopoverHarness {
    open: bool,
    trigger_focus: FocusHandle,
}

impl Render for PopoverHarness {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let harness = cx.entity();
        let trigger_focus = self.trigger_focus.clone();

        v_flex().size_full().p_4().child(
            Popover::new("focus-popover")
                .open(self.open)
                .on_change(move |open, _window, cx| {
                    harness.update(cx, |harness, cx| {
                        harness.open = open;
                        cx.notify();
                    });
                })
                .trigger(move |_, _| {
                    div()
                        .w(px(200.))
                        .h(px(40.))
                        .track_focus(&trigger_focus)
                        .debug_selector(|| "focus-popover-trigger".to_string())
                        .into_any_element()
                })
                .content(|_, _| {
                    div()
                        .h(px(120.))
                        .debug_selector(|| "focus-popover-content".to_string())
                        .into_any_element()
                }),
        )
    }
}

/// The focused handle, whether it is on screen, and the trigger the popover opened over.
fn focus_state(cx: &mut VisualTestContext) -> Option<(FocusHandle, bool)> {
    cx.update(|window, cx| {
        window
            .focused(cx)
            .map(|focused| (focused.clone(), focused.is_rendered(window)))
    })
}

#[gpui::test]
fn closing_a_popover_restores_the_previous_focus(cx: &mut TestAppContext) {
    cx.update(Theme::init);
    let window = cx.open_window(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), |_, cx| {
        PopoverHarness {
            open: false,
            trigger_focus: cx.focus_handle(),
        }
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();

    let harness = window.root(&mut cx).expect("harness view");
    let trigger_focus = cx.update(|_, cx| harness.read(cx).trigger_focus.clone());
    let trigger = cx
        .debug_bounds("focus-popover-trigger")
        .expect("trigger laid out");

    cx.update(|window, cx| window.focus(&trigger_focus, cx));
    assert_eq!(
        focus_state(&mut cx),
        Some((trigger_focus.clone(), true)),
        "the trigger holds focus before the popover opens"
    );

    cx.simulate_click(trigger.center(), Modifiers::default());
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("popover-panel").is_some(),
        "clicking the trigger opens the popover"
    );
    let (opened_focus, opened_rendered) = focus_state(&mut cx).expect("the panel takes focus");
    assert!(opened_rendered, "the open panel's focus is on screen");
    assert_ne!(
        opened_focus, trigger_focus,
        "the panel holds focus while open"
    );

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("popover-panel").is_none(),
        "escape closes the popover"
    );
    assert_eq!(
        focus_state(&mut cx),
        Some((trigger_focus, true)),
        "focus returns to the trigger"
    );
}
