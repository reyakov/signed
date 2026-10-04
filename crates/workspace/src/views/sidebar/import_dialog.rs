use gpui::prelude::*;
use gpui::{AnyWindowHandle, App, Entity, Subscription, Task, Window, px};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::dialog::{DialogDescription, DialogFooter, DialogHeader, DialogTitle};
use gpui_component::form::{field, v_form};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Disableable, WindowExt};
use signed_state::Backend;

use crate::views::dialog_state::{DialogProgress, error_row};

#[derive(Clone, Copy, Default)]
enum Stage {
    #[default]
    Credential,
    /// An `nsec` was entered; ask for a passphrase to encrypt it with.
    Encrypt,
    /// An `ncryptsec` was entered; ask for its passphrase to decrypt it.
    Decrypt,
}

/// The credential entered on the first step.
enum Credential {
    Nsec(String),
    Ncryptsec(String),
    Bunker(String),
}

impl Credential {
    fn parse(input: &str) -> Option<Self> {
        let input = input.trim();

        if input.starts_with("nsec1") {
            Some(Self::Nsec(input.to_owned()))
        } else if input.starts_with("ncryptsec1") {
            Some(Self::Ncryptsec(input.to_owned()))
        } else if input.starts_with("bunker://") {
            Some(Self::Bunker(input.to_owned()))
        } else {
            None
        }
    }
}

/// State of the import dialog, so the current step and async results can be rendered.
#[derive(Default)]
pub struct ImportState {
    stage: Stage,
    credential: String,
    progress: DialogProgress,
    /// Keeps the subscriptions alive while the dialog is open.
    subscriptions: Vec<Subscription>,
}

pub fn open(window: &mut Window, cx: &mut App) {
    let secret_input =
        cx.new(|cx| InputState::new(window, cx).placeholder("secret key or bunker://"));

    let pass_input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder("Passphrase to protect your key")
            .masked(true)
    });

    let repass_input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder("Repeat passphrase")
            .masked(true)
    });

    let handle = window.window_handle();
    let state = cx.new(|_| ImportState::default());

    // Enter in any field submits the step the dialog is on.
    let mut subscriptions = Vec::new();

    for input in [&secret_input, &pass_input, &repass_input] {
        let secret_input = secret_input.clone();
        let pass_input = pass_input.clone();
        let repass_input = repass_input.clone();
        let state = state.clone();

        subscriptions.push(cx.subscribe(input, move |_input, event, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                submit(
                    &secret_input,
                    &pass_input,
                    &repass_input,
                    &state,
                    &handle,
                    cx,
                );
            }
        }));
    }

    state.update(cx, |state, _cx| {
        state.subscriptions = subscriptions;
    });

    window.open_dialog(cx, move |dialog, _window, _cx| {
        let secret_input = secret_input.clone();
        let pass_input = pass_input.clone();
        let repass_input = repass_input.clone();
        let state = state.clone();

        dialog
            .width(px(520.))
            .margin_top(px(50.))
            .content(move |content, _window, cx| {
                let stage = state.read(cx).stage;
                let busy = state.read(cx).progress.busy;
                let error = state.read(cx).progress.error.clone();

                content
                    .child(
                        DialogHeader::new()
                            .child(DialogTitle::new().child("Import identity"))
                            .child(DialogDescription::new().child(match stage {
                                Stage::Credential => {
                                    "Paste the secret key or bunker URI of an existing identity."
                                }
                                Stage::Encrypt => {
                                    "Choose a passphrase to encrypt your key on this device."
                                }
                                Stage::Decrypt => "Enter the passphrase used to encrypt this key.",
                            })),
                    )
                    .child(match stage {
                        Stage::Credential => v_form().child(
                            field()
                                .label("Secret key or bunker URI")
                                .required(true)
                                .child(Input::new(&secret_input)),
                        ),
                        Stage::Encrypt => v_form()
                            .child(
                                field()
                                    .label("Passphrase")
                                    .required(true)
                                    .child(Input::new(&pass_input)),
                            )
                            .child(field().required(true).child(Input::new(&repass_input))),
                        Stage::Decrypt => v_form().child(
                            field()
                                .label("Passphrase")
                                .required(true)
                                .child(Input::new(&pass_input)),
                        ),
                    })
                    .children(error_row(&error, cx))
                    .child(
                        DialogFooter::new().justify_end().child(
                            Button::new("import")
                                .primary()
                                .label(match stage {
                                    Stage::Credential => "Continue",
                                    Stage::Encrypt => "Import identity",
                                    Stage::Decrypt => "Unlock",
                                })
                                .loading(busy)
                                .disabled(busy)
                                .on_click({
                                    let secret_input = secret_input.clone();
                                    let pass_input = pass_input.clone();
                                    let repass_input = repass_input.clone();
                                    let state = state.clone();

                                    move |_ev, _window, cx| {
                                        submit(
                                            &secret_input,
                                            &pass_input,
                                            &repass_input,
                                            &state,
                                            &handle,
                                            cx,
                                        );
                                    }
                                }),
                        ),
                    )
            })
    });
}

fn submit(
    secret_input: &Entity<InputState>,
    pass_input: &Entity<InputState>,
    repass_input: &Entity<InputState>,
    state: &Entity<ImportState>,
    handle: &AnyWindowHandle,
    cx: &mut App,
) {
    if state.read(cx).progress.busy {
        return;
    }

    match state.read(cx).stage {
        Stage::Credential => start_import(secret_input, state, handle, cx),
        Stage::Encrypt => import_nsec(pass_input, repass_input, state, handle, cx),
        Stage::Decrypt => import_ncryptsec(pass_input, state, handle, cx),
    }
}

/// Parses the entered credential and either moves to the passphrase step or connects to a bunker.
fn start_import(
    secret_input: &Entity<InputState>,
    state: &Entity<ImportState>,
    handle: &AnyWindowHandle,
    cx: &mut App,
) {
    let credential = match Credential::parse(&secret_input.read(cx).value()) {
        Some(credential) => credential,
        None => {
            state.update(cx, |state, _| {
                state
                    .progress
                    .fail("Enter an nsec, ncryptsec or bunker:// URI");
            });
            return;
        }
    };

    match credential {
        Credential::Nsec(secret) => {
            state.update(cx, |state, _| {
                state.credential = secret;
                state.stage = Stage::Encrypt;
                state.progress.error = None;
            });
        }
        Credential::Ncryptsec(secret) => {
            state.update(cx, |state, _| {
                state.credential = secret;
                state.stage = Stage::Decrypt;
                state.progress.error = None;
            });
        }
        Credential::Bunker(uri) => {
            state.update(cx, |state, _| state.progress.begin());

            let backend = Backend::global(cx);
            let task = backend.update(cx, |backend, cx| backend.import_bunker(&uri, cx));
            finish_import(task, state, handle, cx);
        }
    }
}

/// Encrypts the entered `nsec` with the passphrase and signs in with it.
fn import_nsec(
    pass_input: &Entity<InputState>,
    repass_input: &Entity<InputState>,
    state: &Entity<ImportState>,
    handle: &AnyWindowHandle,
    cx: &mut App,
) {
    let backend = Backend::global(cx);
    let nsec = state.read(cx).credential.clone();
    let pass = pass_input.read(cx).value().to_string();
    let repass = repass_input.read(cx).value().to_string();

    if pass.is_empty() {
        state.update(cx, |state, _| {
            state.progress.fail("Passphrase must not be empty");
        });
        return;
    }

    if pass != repass {
        state.update(cx, |state, _| {
            state.progress.fail("Passphrases do not match");
        });
        return;
    }

    state.update(cx, |state, _cx| {
        state.progress.begin();
    });

    let task = backend.update(cx, |backend, cx| backend.import_nsec(&nsec, &pass, cx));
    finish_import(task, state, handle, cx);
}

/// Decrypts the entered `ncryptsec` with the passphrase and signs in with it.
fn import_ncryptsec(
    pass_input: &Entity<InputState>,
    state: &Entity<ImportState>,
    handle: &AnyWindowHandle,
    cx: &mut App,
) {
    let backend = Backend::global(cx);
    let ncryptsec = state.read(cx).credential.clone();
    let pass = pass_input.read(cx).value().to_string();

    if pass.is_empty() {
        state.update(cx, |state, _| {
            state.progress.fail("Passphrase must not be empty");
        });
        return;
    }

    state.update(cx, |state, _cx| {
        state.progress.begin();
    });

    let task = backend.update(cx, |backend, cx| {
        backend.import_ncryptsec(&ncryptsec, &pass, cx)
    });
    finish_import(task, state, handle, cx);
}

/// Runs the import task to completion, closing the dialog on success.
fn finish_import<T: 'static>(
    task: Task<Result<T, anyhow::Error>>,
    state: &Entity<ImportState>,
    handle: &AnyWindowHandle,
    cx: &mut App,
) {
    let handle = *handle;
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
                state.update(cx, |state, _| state.progress.fail(e.to_string()));
            })
            .ok();
        }
    })
    .detach();
}
