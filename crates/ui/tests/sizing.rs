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
    let icon_only = cx
        .debug_bounds("icon-only-dropdown-trigger")
        .expect("icon-only dropdown laid out");

    assert_eq!(medium.size.height, px(32.0));
    assert_eq!(xsmall.size.height, px(20.0));
    assert_eq!(icon_only.size, size(px(32.0), px(32.0)));
}
