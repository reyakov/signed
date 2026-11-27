use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Task, Window, div, px,
};
use signed_state::Backend;
use ui::button::Button;
use ui::common::v_flex;
use ui::dialog::{Dialog, DialogHandle};
use ui::field::Field;
use ui::icons::{Icon, IconRole};
use ui::input::{Input, InputState, InputStateEvent};
use ui::theme::ActiveTheme as _;

use crate::shared::log_dropped;

pub struct PassphraseDialog {
    handle: DialogHandle,
    pass_input: Entity<InputState>,
    busy: bool,
    error: Option<SharedString>,
    task: Option<Task<()>>,
    _enter_subscription: Subscription,
}

impl PassphraseDialog {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let pass_input = InputState::builder().password().build(cx);

        cx.new(|cx| {
            let enter_subscription =
                cx.subscribe(&pass_input, |this: &mut Self, _input, event, cx| {
                    if matches!(event, InputStateEvent::Submitted) {
                        this.unlock(cx);
                    }
                });

            Self {
                handle: DialogHandle::new(true),
                pass_input,
                busy: false,
                error: None,
                task: None,
                _enter_subscription: enter_subscription,
            }
        })
    }

    fn unlock(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }

        let pass = self.pass_input.read(cx).value().to_string();
        if pass.is_empty() {
            self.busy = false;
            self.error = Some("Passphrase must not be empty".into());
            cx.notify();
            return;
        }

        self.busy = true;
        self.error = None;

        let backend = Backend::global(cx);
        let task = backend.update(cx, |backend, cx| backend.restore_with_passphrase(&pass, cx));
        let handle = self.handle.clone();

        self.task = Some(cx.spawn(async move |this, cx| match task.await {
            Ok(_) => {
                cx.update(|cx| handle.close(cx));
                log_dropped(this.update(cx, |this, cx| {
                    this.busy = false;
                    cx.notify();
                }));
            }
            Err(error) => {
                log_dropped(this.update(cx, |this, cx| {
                    this.busy = false;
                    this.error = Some(error.to_string().into());
                    cx.notify();
                }));
            }
        }));
    }
}

impl Render for PassphraseDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pass_input = self.pass_input.clone();
        let error = self.error.clone();
        let this = cx.weak_entity();
        let busy = self.busy;

        Dialog::new("passphrase-dialog")
            .handle(self.handle.clone())
            .close_on_escape(false)
            .close_on_backdrop(false)
            .min_width(px(420.))
            .title("Unlock your identity")
            .content(move |_window, cx| {
                v_flex()
                    .gap_3()
                    .child(
                        Field::new(
                            "passphrase-field",
                            "Enter the passphrase used to encrypt this identity.",
                        )
                        .child(Input::new("passphrase-input", pass_input.clone())),
                    )
                    .when_some(error.clone(), |el, error| {
                        el.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().ramps.destructive.base)
                                .child(error),
                        )
                    })
                    .into_any_element()
            })
            .actions(move |_, _cx| {
                let this = this.clone();

                vec![
                    Button::new("unlock")
                        .label("Unlock")
                        .icon(Icon::named(IconRole::Unlock))
                        .loading(busy)
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            log_dropped(this.update(cx, |this, cx| {
                                this.unlock(cx);
                            }));
                        })
                        .into_any_element(),
                ]
            })
    }
}
