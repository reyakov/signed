//! An outside click closes an open Select and leaves focus on the click target.

use gpui::{
    Context, FocusHandle, InteractiveElement as _, IntoElement, Modifiers, ParentElement as _,
    Render, Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
};
use ui::common::v_flex;
use ui::select::{Select, SelectOption};
use ui::theme::Theme;

const WINDOW_WIDTH: f32 = 800.0;
const WINDOW_HEIGHT: f32 = 1000.0;

/// A select above a focusable element, so the open panel never covers the target.
struct SelectHarness {
    value: Option<String>,
    outside_focus: FocusHandle,
}

impl Render for SelectHarness {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let outside_focus = self.outside_focus.clone();

        v_flex()
            .size_full()
            .p_4()
            .child(
                Select::new("s")
                    .options(
                        ["alpha", "beta"].map(|value| SelectOption::new(value.to_string(), value)),
                    )
                    .placeholder("Pick")
                    .on_change(cx.processor(|this, value: Option<String>, _, cx| {
                        this.value = value.clone();
                        cx.notify();
                    })),
            )
            .child(div().h(px(500.)))
            .child(
                div()
                    .id("outside")
                    .track_focus(&outside_focus)
                    .debug_selector(|| "outside".to_string())
                    .h(px(40.))
                    .w(px(200.)),
            )
    }
}

#[gpui::test]
fn outside_click_leaves_focus_on_the_target(cx: &mut TestAppContext) {
    cx.update(|cx| Theme::init_with(cx, Theme::fallback_source()));
    let window = cx.open_window(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), |_, cx| {
        SelectHarness {
            value: None,
            outside_focus: cx.focus_handle(),
        }
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    let harness = window.root(&mut cx).expect("harness view");
    let outside_focus = cx.update(|_, cx| harness.read(cx).outside_focus.clone());
    let trigger = cx.debug_bounds("select-trigger").expect("trigger laid out");

    cx.simulate_click(trigger.center(), Modifiers::default());
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("menu-panel").is_some(),
        "clicking the trigger opens the options"
    );

    let outside = cx.debug_bounds("outside").expect("target laid out");
    cx.simulate_click(outside.center(), Modifiers::default());
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("menu-panel").is_none(),
        "the outside click closes the options"
    );
    assert_eq!(
        cx.update(|window, cx| window.focused(cx)),
        Some(outside_focus),
        "focus stays on the clicked element, not the trigger"
    );
    assert_eq!(
        harness.read_with(&cx, |harness, _| harness.value.clone()),
        None,
        "an outside click chooses nothing"
    );
}
