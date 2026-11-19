use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div,
};
use signed_state::Backend;
use ui::button::{Button, ButtonVariant};
use ui::common::v_flex;
use ui::dialog::{Dialog, DialogHandle};
use ui::theme::{ActiveTheme as _, TextRole, Typeset as _};

pub struct ExitDialog {
    handle: DialogHandle,
}

impl ExitDialog {
    pub fn new(cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self {
            handle: DialogHandle::new(true),
        })
    }
}

impl Render for ExitDialog {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let cancel_close = self.handle.clone();
        let confirm_close = self.handle.clone();

        Dialog::new("exit-dialog")
            .handle(self.handle.clone())
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
            .actions(move |_, _| {
                let cancel_close = cancel_close.clone();
                let confirm_close = confirm_close.clone();

                let cancel: AnyElement = Button::new("cancel-exit")
                    .label("Cancel")
                    .variant(ButtonVariant::Secondary)
                    .on_click(move |_, _, cx| cancel_close.close(cx))
                    .into_any_element();

                let exit: AnyElement = Button::new("confirm-exit")
                    .label("Exit")
                    .variant(ButtonVariant::Destructive)
                    .on_click(move |_, _, cx| {
                        let backend = Backend::global(cx);
                        backend.update(cx, |backend, cx| backend.sign_out(cx));
                        confirm_close.close(cx);
                    })
                    .into_any_element();

                vec![cancel, exit]
            })
    }
}
