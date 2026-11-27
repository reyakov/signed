//! Menu panels render only their visible rows and dispatch navigation keys as actions.

use gpui::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, px, size,
};
use ui::common::{ResultLogErr as _, v_flex};
use ui::menu::{Menu, MenuItem};
use ui::theme::Theme;

const WINDOW_WIDTH: f32 = 800.0;
const WINDOW_HEIGHT: f32 = 1000.0;
const ROWS: usize = 300;

/// A menu of `ROWS` action rows, with its open state held by the view.
struct MenuHarness {
    open: bool,
    activated: Vec<usize>,
    changes: Vec<bool>,
}

impl Render for MenuHarness {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();

        v_flex().size_full().p_4().child(
            Menu::new("m")
                .open(self.open)
                .on_change(cx.processor(|this, open: bool, _, cx| {
                    this.open = open;
                    this.changes.push(open);
                    cx.notify();
                }))
                .trigger(|_, _| {
                    div()
                        .h(px(24.))
                        .w(px(120.))
                        .debug_selector(|| "m-trigger".to_string())
                        .into_any_element()
                })
                .items(move |_, _| {
                    (0..ROWS)
                        .map(|index| {
                            let activated = weak.clone();
                            MenuItem::new(format!("Item {index}")).handler(move |_, cx| {
                                activated
                                    .update(cx, |harness, cx| {
                                        harness.activated.push(index);
                                        cx.notify();
                                    })
                                    .log_err();
                            })
                        })
                        .collect()
                }),
        )
    }
}

/// Opens the harness's menu with its recordings cleared, and parks the frame.
fn open(cx: &mut VisualTestContext, harness: &gpui::Entity<MenuHarness>) {
    harness.update(cx, |harness, cx| {
        harness.open = true;
        harness.activated.clear();
        harness.changes.clear();
        cx.notify();
    });
    // Draw once so the panel's paint-time focus grab runs before the keys arrive.
    cx.update(|_, _| {});
    cx.run_until_parked();
    // Draw twice: the first paint defers the focus grab, the second registers the
    // panel's focus path so dispatched keys reach it.
    cx.update(|_, _| {});
    cx.run_until_parked();
    cx.update(|_, _| {});
    cx.run_until_parked();
}

#[gpui::test]
fn menu_rows_are_virtualized(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Theme::init_with(cx, Theme::fallback_source());
        Menu::bind_keys(cx);
    });
    let window = cx.open_window(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), |_, _| {
        MenuHarness {
            open: false,
            activated: Vec::new(),
            changes: Vec::new(),
        }
    });
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();
    let harness = window.root(&mut cx).expect("harness view");

    open(&mut cx, &harness);

    assert!(
        cx.debug_bounds("menu-item-0").is_some(),
        "the first row is on screen"
    );
    assert!(
        cx.debug_bounds("menu-item-299").is_none(),
        "rows beyond the panel's height are never laid out"
    );
}
