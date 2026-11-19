use gpui::{
    Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px, size,
};
use ui::button::Button;
use ui::common::Sizable as _;
use ui::dropdown_menu::DropdownMenu;
use ui::icons::IconRole;
use ui::menu::MenuItem;
use ui::theme::Theme;

struct Harness;

impl Render for Harness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                Button::new("height-probe")
                    .label("Probe")
                    .debug_selector(|| "height-probe".to_string()),
            )
            .child(
                Button::new("xsmall-height-probe")
                    .label("Probe")
                    .xsmall()
                    .debug_selector(|| "xsmall-height-probe".to_string()),
            )
            .child(
                Button::new("small-height-probe")
                    .label("Probe")
                    .small()
                    .debug_selector(|| "small-height-probe".to_string()),
            )
            .child(
                Button::new("large-height-probe")
                    .label("Probe")
                    .large()
                    .debug_selector(|| "large-height-probe".to_string()),
            )
            .child(
                Button::new("child-only-probe")
                    .fully_rounded(true)
                    .padded(false)
                    .child(div().size(px(24.)))
                    .debug_selector(|| "child-only-probe".to_string()),
            )
            .child(
                DropdownMenu::new("icon-only-dropdown")
                    .icon(IconRole::More)
                    .accessibility_label("More actions")
                    .items(|_, _| vec![MenuItem::new("One")]),
            )
    }
}

#[gpui::test]
fn buttons_render_at_their_token_heights(cx: &mut TestAppContext) {
    cx.update(Theme::init);
    let window = cx.open_window(size(px(400.), px(400.)), |_, _| Harness);
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();

    let medium = cx.debug_bounds("height-probe").expect("button laid out");
    let xsmall = cx
        .debug_bounds("xsmall-height-probe")
        .expect("xsmall button laid out");
    let small = cx
        .debug_bounds("small-height-probe")
        .expect("small button laid out");
    let large = cx
        .debug_bounds("large-height-probe")
        .expect("large button laid out");
    let icon_only = cx
        .debug_bounds("icon-only-dropdown-trigger")
        .expect("icon-only dropdown laid out");
    let child_only = cx
        .debug_bounds("child-only-probe")
        .expect("child-only button laid out");

    assert_eq!(medium.size.height, px(32.0));
    assert_eq!(xsmall.size.height, px(20.0));
    assert_eq!(small.size.height, px(28.0));
    assert_eq!(large.size.height, px(40.0));
    assert_eq!(icon_only.size, size(px(32.0), px(32.0)));
    assert_eq!(child_only.size, size(px(32.0), px(32.0)));
}
