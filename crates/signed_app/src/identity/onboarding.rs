use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Task, Window, div, px,
};
use signed_state::Backend;
use ui::button::Button;
use ui::common::v_flex;
use ui::dialog::{Dialog, DialogHandle};
use ui::field::Field;
use ui::input::{Input, InputState};
use ui::theme::ActiveTheme as _;

use crate::shared::log_dropped;

pub struct OnboardingDialog {
    handle: DialogHandle,
    name_input: Entity<InputState>,
    pass_input: Entity<InputState>,
    repass_input: Entity<InputState>,
    busy: bool,
    error: Option<SharedString>,
    task: Option<Task<()>>,
}

impl OnboardingDialog {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let name_input = InputState::new(cx);
        let pass_input = InputState::builder().password().build(cx);
        let repass_input = InputState::builder().password().build(cx);

        cx.new(|_| Self {
            handle: DialogHandle::new(true),
            name_input,
            pass_input,
            repass_input,
            busy: false,
            error: None,
            task: None,
        })
    }

    fn create_identity(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }

        let name = self.name_input.read(cx).value().to_string();
        let pass = self.pass_input.read(cx).value().to_string();
        let repass = self.repass_input.read(cx).value().to_string();

        if pass != repass {
            self.busy = false;
            self.error = Some("Passphrases do not match".into());
            cx.notify();
            return;
        }

        self.busy = true;
        self.error = None;

        let backend = Backend::global(cx);
        let task = backend.update(cx, |backend, cx| backend.create_identity(&name, &pass, cx));
        let handle = self.handle.clone();

        self.task = Some(cx.spawn(async move |this, cx| {
            match task.await {
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
            };
        }));
    }
}

impl Render for OnboardingDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        const INTRO: &str = "Set up your Signed identity to get started.";

        let name_input = self.name_input.clone();
        let pass_input = self.pass_input.clone();
        let repass_input = self.repass_input.clone();
        let error = self.error.clone();
        let busy = self.busy;

        let this = cx.weak_entity();

        Dialog::new("onboarding-dialog")
            .handle(self.handle.clone())
            .close_on_escape(!busy)
            .close_on_backdrop(!busy)
            .min_width(px(520.))
            .title("Create identity")
            .content(move |_window, cx| {
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child(INTRO),
                    )
                    .child(
                        Field::new("name-field", "Name")
                            .description("Max 255 characters")
                            .child(Input::new("name-input", name_input.clone())),
                    )
                    .child(
                        Field::new("passphrase-field", "Passphrase")
                            .child(Input::new("passphrase-input", pass_input.clone())),
                    )
                    .child(
                        Field::new("repeat-field", "Repeat passphrase")
                            .child(Input::new("repeat-input", repass_input.clone())),
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
                    Button::new("create-identity")
                        .label("Create new identity")
                        .loading(busy)
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            log_dropped(this.update(cx, |this, cx| this.create_identity(cx)));
                        })
                        .into_any_element(),
                ]
            })
    }
}
