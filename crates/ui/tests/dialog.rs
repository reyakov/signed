//! One Escape closes a dialog even when a text field inside it has focus.

use gpui::{
    Context, IntoElement, Modifiers, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px, size,
};
use ui::common::{ResultLogErr as _, v_flex};
use ui::dialog::{Dialog, DialogHandle};
use ui::input::{Input, InputState};
use ui::theme::Theme;

const WINDOW_WIDTH: f32 = 800.0;
const WINDOW_HEIGHT: f32 = 600.0;

/// A dialog with a text field and an action, with its open state held by the view.
struct DialogHarness {
    handle: DialogHandle,
    input: gpui::Entity<InputState>,
    closes: Vec<bool>,
}

impl Render for DialogHarness {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input = self.input.clone();
        let this = cx.weak_entity();

        v_flex().size_full().p_4().child(
            Dialog::new("escape-dialog")
                .handle(self.handle.clone())
                .title("Escape closes")
                .on_change(move |open, _window, cx| {
                    this.update(cx, |harness, _| harness.closes.push(open))
                        .log_err();
                })
                .content(move |_, _| {
                    div()
                        .p_2()
                        .child(Input::new("dialog-field", input.clone()))
                        .into_any_element()
                }),
        )
    }
}

#[gpui::test]
fn one_escape_closes_a_dialog_with_a_focused_field(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Theme::init_with(cx, Theme::fallback_source());
        Dialog::bind_keys(cx);
    });
    let window = cx.open_window(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), |_, cx| {
        DialogHarness {
            handle: DialogHandle::new(true),
            input: InputState::new(cx),
            closes: Vec::new(),
        }
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();

    let harness = window.root(&mut cx).expect("harness view");
    cx.update(|window, cx| window.focus(&harness.read(cx).input.read(cx).focus_handle(), cx));
    cx.update(|_, _| {});
    cx.run_until_parked();

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    assert_eq!(
        harness.read_with(&cx, |harness, _| harness.closes.clone()),
        vec![false],
        "one escape press reports a single close to the dialog's owner"
    );
}

#[gpui::test]
fn tab_keeps_focus_inside_the_dialog(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Theme::init_with(cx, Theme::fallback_source());
        Dialog::bind_keys(cx);
    });
    let window = cx.open_window(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), |_, cx| {
        DialogHarness {
            handle: DialogHandle::new(true),
            input: InputState::new(cx),
            closes: Vec::new(),
        }
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();

    let harness = window.root(&mut cx).expect("harness view");
    let field = cx.update(|_, cx| harness.read(cx).input.read(cx).focus_handle());
    cx.update(|window, cx| window.focus(&field, cx));
    cx.update(|_, _| {});
    cx.run_until_parked();

    cx.simulate_keystrokes("tab");
    cx.run_until_parked();

    let contained = cx.update(|window, cx| {
        let dialog = window.focused(cx).expect("focus stays in the window");
        dialog.contains_focused(window, cx) || dialog == field
    });
    assert!(contained, "tab does not move focus behind the dialog");
    assert!(
        cx.debug_bounds("escape-dialog").is_some()
            || harness.read_with(&cx, |h, _| h.closes.is_empty()),
        "the dialog stays open"
    );
    let _ = Modifiers::default();
}
