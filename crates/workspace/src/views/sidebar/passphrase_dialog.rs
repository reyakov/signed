use assets::CustomIconName;
use gpui::prelude::*;
use gpui::{AnyWindowHandle, App, Entity, Subscription, Window};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dialog::{DialogDescription, DialogFooter, DialogHeader, DialogTitle};
use gpui_component::form::{field, v_form};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Disableable, WindowExt};
use signed_state::Backend;

use crate::views::dialog_state::{DialogProgress, error_row};

/// State of the passphrase dialog, so async results can be rendered.
#[derive(Default)]
pub struct PassphraseState {
    pub progress: DialogProgress,
    /// Keeps the Enter-to-submit subscription alive while the dialog is open.
    _enter_subscription: Option<Subscription>,
}

pub fn open(window: &mut Window, cx: &mut App) {
    let pass_input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder("Passphrase to unlock your identity")
            .masked(true)
    });

    let handle = window.window_handle();
    let state = cx.new(|_| PassphraseState::default());

    // Enter in the passphrase field submits, like the Unlock button.
    let enter_pass_input = pass_input.clone();
    let enter_state = state.clone();
    let enter_subscription = cx.subscribe(&pass_input, move |_input, event, cx| {
        if matches!(event, InputEvent::PressEnter { .. }) {
            unlock(&enter_pass_input, &enter_state, &handle, cx);
        }
    });

    state.update(cx, |state, _| {
        state._enter_subscription = Some(enter_subscription)
    });

    window.open_dialog(cx, move |dialog, _window, _cx| {
        let pass_input = pass_input.clone();
        let state = state.clone();

        dialog
            .close_button(false)
            .overlay_closable(false)
            .keyboard(false)
            .content(move |content, _window, cx| {
                let busy = state.read(cx).progress.busy;
                let error = state.read(cx).progress.error.clone();

                content
                    .child(
                        DialogHeader::new()
                            .child(DialogTitle::new().child("Unlock your identity"))
                            .child(
                                DialogDescription::new()
                                    .child("Enter the passphrase used to encrypt this identity."),
                            ),
                    )
                    .child(
                        v_form().child(
                            field()
                                .label("Passphrase")
                                .required(true)
                                .child(Input::new(&pass_input)),
                        ),
                    )
                    .children(error_row(&error, cx))
                    .child(
                        DialogFooter::new().justify_end().child(
                            Button::new("unlock")
                                .primary()
                                .icon(CustomIconName::Unlock)
                                .label("Unlock")
                                .loading(busy)
                                .disabled(busy)
                                .on_click({
                                    let pass_input = pass_input.clone();
                                    let state = state.clone();

                                    move |_ev, _window, cx| {
                                        unlock(&pass_input, &state, &handle, cx);
                                    }
                                }),
                        ),
                    )
            })
    });
}

fn unlock(
    pass_input: &Entity<InputState>,
    state: &Entity<PassphraseState>,
    handle: &AnyWindowHandle,
    cx: &mut App,
) {
    let backend = Backend::global(cx);
    let pass = pass_input.read(cx).value().to_string();

    if pass.is_empty() {
        state.update(cx, |state, _| {
            state.progress.fail("Passphrase must not be empty");
        });
        return;
    }

    state.update(cx, |state, _| state.progress.begin());

    let task = backend.update(cx, |backend, cx| backend.restore_with_passphrase(&pass, cx));
    let handle = *handle;
    let state = state.clone();

    cx.spawn(async move |cx| match task.await {
        Ok(_) => {
            cx.update_window(handle, |_this, window, cx| {
                window.close_dialog(cx);
            })
            .ok();
        }
        Err(e) => {
            cx.update_window(handle, |_this, _window, cx| {
                state.update(cx, |state, _| state.progress.fail(e.to_string()));
            })
            .ok();
        }
    })
    .detach();
}
