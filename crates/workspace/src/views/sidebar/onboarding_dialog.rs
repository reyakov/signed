use gpui::prelude::*;
use gpui::{App, Entity, Window, px};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dialog::{DialogDescription, DialogFooter, DialogHeader, DialogTitle};
use gpui_component::form::{field, v_form};
use gpui_component::input::{Input, InputState};
use gpui_component::{Disableable, WindowExt};
use signed_state::Backend;

use crate::views::dialog_state::{DialogProgress, error_row};

/// Progress of the onboarding flow, so async results can be rendered.
pub type OnboardingState = DialogProgress;

pub fn open(
    name_input: Entity<InputState>,
    pass_input: Entity<InputState>,
    repass_input: Entity<InputState>,
    state: Entity<OnboardingState>,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_dialog(cx, move |dialog, _window, _cx| {
        let name_input = name_input.clone();
        let pass_input = pass_input.clone();
        let repass_input = repass_input.clone();
        let state = state.clone();

        dialog
            .width(px(520.))
            .margin_top(px(50.))
            .content(move |content, _window, cx| {
                let busy = state.read(cx).busy;
                let error = state.read(cx).error.clone();

                content
                    .child(
                        DialogHeader::new()
                            .child(DialogTitle::new().child("Create identity"))
                            .child(
                                DialogDescription::new()
                                    .child("Set up your Signed identity to get started."),
                            ),
                    )
                    .child(
                        v_form()
                            .child(
                                field()
                                    .label("Name")
                                    .description("Max 255 characters")
                                    .required(true)
                                    .child(Input::new(&name_input)),
                            )
                            .child(
                                field()
                                    .label("Passphrase")
                                    .required(true)
                                    .child(Input::new(&pass_input)),
                            )
                            .child(field().required(true).child(Input::new(&repass_input))),
                    )
                    .children(error_row(&error, cx))
                    .child(
                        DialogFooter::new().justify_end().child(
                            Button::new("continue")
                                .primary()
                                .label("Create new identity")
                                .tooltip("Create identity")
                                .loading(busy)
                                .disabled(busy)
                                .on_click({
                                    let name_input = name_input.clone();
                                    let pass_input = pass_input.clone();
                                    let repass_input = repass_input.clone();
                                    let state = state.clone();

                                    move |_ev, window, cx| {
                                        let backend = Backend::global(cx);
                                        let name = name_input.read(cx).value().to_string();
                                        let pass = pass_input.read(cx).value().to_string();
                                        let repass = repass_input.read(cx).value().to_string();

                                        if pass != repass {
                                            state.update(cx, |state, _| {
                                                state.fail("Passphrases do not match");
                                            });
                                            return;
                                        }

                                        state.update(cx, |state, _| state.begin());

                                        let task = backend.update(cx, |backend, cx| {
                                            backend.create_identity(&name, &pass, cx)
                                        });
                                        let handle = window.window_handle();
                                        let state = state.clone();

                                        cx.spawn(async move |cx| match task.await {
                                            Ok(_) => {
                                                cx.update_window(handle, |_, window, cx| {
                                                    window.close_dialog(cx);
                                                })
                                                .ok();
                                            }
                                            Err(e) => {
                                                cx.update_window(handle, |_, _window, cx| {
                                                    state.update(cx, |state, _| {
                                                        state.fail(e.to_string());
                                                    });
                                                })
                                                .ok();
                                            }
                                        })
                                        .detach();
                                    }
                                }),
                        ),
                    )
            })
    });
}
