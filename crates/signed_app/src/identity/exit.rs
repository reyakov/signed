use gpui::{
    AnyElement, Context, IntoElement, ParentElement as _, Render, Styled as _, Window, div,
};
use gpui_native_kit::button::{Button, ButtonVariant};
use gpui_native_kit::common::v_flex;
use gpui_native_kit::dialog::Dialog;
use gpui_native_kit::theme::{ActiveTheme as _, TextRole, Typeset as _};
use signed_state::Backend;

use crate::shared::dialogs::close_global;

pub struct ExitDialog;

impl Render for ExitDialog {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Dialog::new("exit-dialog")
            .open(true)
            .on_change(|open, _, cx| {
                if !open {
                    close_global(cx);
                }
            })
            .title("Exit Signed?")
            .content(|_, cx| {
                v_flex()
                    .child(
                        div()
                            .typeset(TextRole::Body, cx)
                            .text_color(cx.theme().colors.muted_foreground)
                            .child(
                                "You will be signed out on this device and will need to sign in again on the next session.",
                            ),
                    )
                    .into_any_element()
            })
            .actions(|_, _| {
                let cancel: AnyElement = Button::new("cancel-exit")
                    .label("Cancel")
                    .variant(ButtonVariant::Secondary)
                    .on_click(|_, _, cx| close_global(cx))
                    .into_any_element();

                let exit: AnyElement = Button::new("confirm-exit")
                    .label("Exit")
                    .variant(ButtonVariant::Destructive)
                    .on_click(|_, _, cx| {
                        let backend = Backend::global(cx);
                        backend.update(cx, |backend, cx| backend.sign_out(cx));
                        close_global(cx);
                    })
                    .into_any_element();

                vec![cancel, exit]
            })
    }
}
