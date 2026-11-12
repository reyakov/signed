use assets::CustomIconName;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, Window, div,
};
use gpui_native_kit::button::Button;
use gpui_native_kit::common::v_flex;
use gpui_native_kit::dialog::Dialog;
use gpui_native_kit::field::Field;
use gpui_native_kit::icons::Icon;
use gpui_native_kit::input::{Input, InputState, InputStateEvent};
use gpui_native_kit::theme::ActiveTheme as _;
use signed_state::Backend;

use crate::shared::dialogs::close_global;
use crate::shared::progress::{DialogProgress, error_row, push_steps_row};

pub struct PassphraseDialog {
    pass_input: Entity<InputState>,
    state: Entity<DialogProgress>,
    _enter_subscription: Subscription,
}

impl PassphraseDialog {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let state = cx.new(|_| DialogProgress::default());

        let pass_input = InputState::new(cx);
        pass_input.update(cx, |state, cx| state.set_password(true, cx));

        cx.new(|cx| {
            let enter_subscription =
                cx.subscribe(&pass_input, |this: &mut Self, _input, event, cx| {
                    if matches!(event, InputStateEvent::Submitted) {
                        this.unlock(cx);
                    }
                });

            Self {
                pass_input,
                state,
                _enter_subscription: enter_subscription,
            }
        })
    }

    fn unlock(&mut self, cx: &mut Context<Self>) {
        if self.state.read(cx).busy {
            return;
        }

        let pass = self.pass_input.read(cx).value().to_string();

        if pass.is_empty() {
            self.state.update(cx, |state, cx| {
                state.fail("Passphrase must not be empty");
                cx.notify();
            });
            return;
        }

        self.state.update(cx, |state, cx| {
            state.begin();
            cx.notify();
        });

        let backend = Backend::global(cx);
        let task = backend.update(cx, |backend, cx| backend.restore_with_passphrase(&pass, cx));
        let state = self.state.clone();

        cx.spawn(async move |_this, cx| match task.await {
            Ok(_) => {
                cx.update(close_global);
            }
            Err(error) => {
                state.update(cx, |state, cx| {
                    state.fail(error.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }
}

impl Render for PassphraseDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pass_input = self.pass_input.clone();
        let state = self.state.clone();
        let action_state = state.clone();
        let this = cx.entity().downgrade();

        Dialog::new("passphrase-dialog")
            .open(true)
            .on_change(|_open, _, _| {})
            .title("Unlock your identity")
            .content(move |_window, cx| {
                let progress = state.read(cx);
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child("Enter the passphrase used to encrypt this identity."),
                    )
                    .child(
                        Field::new("passphrase-field", "Passphrase")
                            .child(Input::new("passphrase-input", pass_input.clone())),
                    )
                    .when_some(error_row(&progress.error, cx), |el, error| el.child(error))
                    .when_some(push_steps_row(&progress.push_steps, cx), |el, steps| {
                        el.child(steps)
                    })
                    .into_any_element()
            })
            .actions(move |_, cx| {
                let busy = action_state.read(cx).busy;
                let this = this.clone();

                let unlock: AnyElement = Button::new("unlock")
                    .label("Unlock")
                    .icon(Icon::named(CustomIconName::Unlock))
                    .loading(busy)
                    .disabled(busy)
                    .on_click(move |_, _, cx| {
                        let _ = this.update(cx, |this, cx| this.unlock(cx));
                    })
                    .into_any_element();

                vec![unlock]
            })
    }
}
