use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div, px,
};
use signed_state::Backend;
use ui::common::v_flex;
use ui::dialog::{Dialog, DialogHandle};
use ui::field::Field;
use ui::input::{Input, InputState};
use ui::theme::ActiveTheme as _;

use crate::shared::progress::{DialogProgress, dialog_submit_button, error_row, run_dialog_task};

pub struct OnboardingDialog {
    handle: DialogHandle,
    name_input: Entity<InputState>,
    pass_input: Entity<InputState>,
    repass_input: Entity<InputState>,
    state: Entity<DialogProgress>,
}

impl OnboardingDialog {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let state = cx.new(|_| DialogProgress::default());
        let name_input = InputState::new(cx);
        let pass_input = InputState::builder().password().build(cx);
        let repass_input = InputState::builder().password().build(cx);

        cx.new(|_| Self {
            handle: DialogHandle::new(true),
            name_input,
            pass_input,
            repass_input,
            state,
        })
    }

    fn create_identity(&mut self, cx: &mut Context<Self>) {
        if self.state.read(cx).busy {
            return;
        }

        let name = self.name_input.read(cx).value().to_string();
        let pass = self.pass_input.read(cx).value().to_string();
        let repass = self.repass_input.read(cx).value().to_string();

        if pass != repass {
            self.state.update(cx, |state, cx| {
                state.fail("Passphrases do not match");
                cx.notify();
            });
            return;
        }

        self.state.update(cx, |state, cx| {
            state.begin();
            cx.notify();
        });

        let backend = Backend::global(cx);
        let task = backend.update(cx, |backend, cx| backend.create_identity(&name, &pass, cx));
        run_dialog_task(task, &self.state, &self.handle, cx);
    }
}

impl Render for OnboardingDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name_input = self.name_input.clone();
        let pass_input = self.pass_input.clone();
        let repass_input = self.repass_input.clone();
        let state = self.state.clone();
        let action_state = state.clone();
        let this = cx.entity().downgrade();
        let busy = state.read(cx).busy;

        Dialog::new("onboarding-dialog")
            .handle(self.handle.clone())
            .close_on_escape(!busy)
            .close_on_backdrop(!busy)
            .min_width(px(520.))
            .title("Create identity")
            .content(move |_window, cx| {
                let progress = state.read(cx);
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_base()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child("Set up your Signed identity to get started."),
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
                    .when_some(error_row(&progress.error, cx), |el, error| el.child(error))
                    .into_any_element()
            })
            .actions(move |_, cx| {
                let busy = action_state.read(cx).busy;
                let this = this.clone();

                let create: AnyElement = dialog_submit_button(
                    "create-identity",
                    "Create new identity",
                    None,
                    busy,
                    &this,
                    |this, _window, cx| {
                        this.create_identity(cx);
                        Ok(())
                    },
                )
                .into_any_element();

                vec![create]
            })
    }
}
