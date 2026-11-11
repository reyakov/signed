use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, Task, Window, div, px,
};
use gpui_native_kit::button::Button;
use gpui_native_kit::common::v_flex;
use gpui_native_kit::dialog::Dialog;
use gpui_native_kit::field::Field;
use gpui_native_kit::input::{Input, InputState, InputStateEvent};
use gpui_native_kit::theme::ActiveTheme as _;
use signed_state::Backend;

use super::close_global;
use super::progress::{DialogProgress, error_row};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
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
}

pub struct ImportDialog {
    secret_input: Entity<InputState>,
    pass_input: Entity<InputState>,
    repass_input: Entity<InputState>,
    state: Entity<ImportState>,
    _subscriptions: Vec<Subscription>,
}

impl ImportDialog {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let secret_input = InputState::new(cx);
        let pass_input = InputState::new(cx);
        pass_input.update(cx, |state, cx| state.set_password(true, cx));
        let repass_input = InputState::new(cx);
        repass_input.update(cx, |state, cx| state.set_password(true, cx));
        let state = cx.new(|_| ImportState::default());

        cx.new(|cx| {
            let mut subscriptions = Vec::new();

            for input in [&secret_input, &pass_input, &repass_input] {
                let secret_input = secret_input.clone();
                let pass_input = pass_input.clone();
                let repass_input = repass_input.clone();
                let state = state.clone();

                subscriptions.push(cx.subscribe(input, move |_this, _input, event, cx| {
                    if matches!(event, InputStateEvent::Submitted) {
                        submit(&secret_input, &pass_input, &repass_input, &state, cx);
                    }
                }));
            }

            Self {
                secret_input,
                pass_input,
                repass_input,
                state,
                _subscriptions: subscriptions,
            }
        })
    }
}

impl Render for ImportDialog {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let secret_input = self.secret_input.clone();
        let pass_input = self.pass_input.clone();
        let repass_input = self.repass_input.clone();
        let state = self.state.clone();
        let action_state = state.clone();
        let action_secret = secret_input.clone();
        let action_pass = pass_input.clone();
        let action_repass = repass_input.clone();

        Dialog::new("import-dialog")
            .open(true)
            .min_width(px(520.))
            .on_change(|open, _, cx| {
                if !open {
                    close_global(cx);
                }
            })
            .title("Import identity")
            .content(move |_window, cx| {
                let state = state.read(cx);
                let stage = state.stage;
                let description = match stage {
                    Stage::Credential => {
                        "Paste the secret key or bunker URI of an existing identity."
                    }
                    Stage::Encrypt => "Choose a passphrase to encrypt your key on this device.",
                    Stage::Decrypt => "Enter the passphrase used to encrypt this key.",
                };

                let form = match stage {
                    Stage::Credential => v_flex().gap_3().child(
                        Field::new("secret-field", "Secret key or bunker URI")
                            .child(Input::new("secret-input", secret_input.clone())),
                    ),
                    Stage::Encrypt => v_flex()
                        .gap_3()
                        .child(
                            Field::new("passphrase-field", "Passphrase")
                                .child(Input::new("passphrase-input", pass_input.clone())),
                        )
                        .child(
                            Field::new("repeat-field", "Repeat passphrase")
                                .child(Input::new("repeat-input", repass_input.clone())),
                        ),
                    Stage::Decrypt => v_flex().gap_3().child(
                        Field::new("passphrase-field", "Passphrase")
                            .child(Input::new("passphrase-input", pass_input.clone())),
                    ),
                };

                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child(description),
                    )
                    .child(form)
                    .when_some(error_row(&state.progress.error, cx), |el, error| {
                        el.child(error)
                    })
                    .into_any_element()
            })
            .actions(move |_, cx| {
                let state = action_state.read(cx);
                let busy = state.progress.busy;
                let label = match state.stage {
                    Stage::Credential => "Continue",
                    Stage::Encrypt => "Import identity",
                    Stage::Decrypt => "Unlock",
                };
                let secret = action_secret.clone();
                let pass = action_pass.clone();
                let repass = action_repass.clone();
                let state = action_state.clone();

                let import: AnyElement = Button::new("import-identity")
                    .label(label)
                    .loading(busy)
                    .disabled(busy)
                    .on_click(move |_, _, cx| {
                        submit(&secret, &pass, &repass, &state, cx);
                    })
                    .into_any_element();

                vec![import]
            })
    }
}

fn submit(
    secret_input: &Entity<InputState>,
    pass_input: &Entity<InputState>,
    repass_input: &Entity<InputState>,
    state: &Entity<ImportState>,
    cx: &mut App,
) {
    if state.read(cx).progress.busy {
        return;
    }

    match state.read(cx).stage {
        Stage::Credential => start_import(secret_input, state, cx),
        Stage::Encrypt => import_nsec(pass_input, repass_input, state, cx),
        Stage::Decrypt => import_ncryptsec(pass_input, state, cx),
    }
}

/// Parses the entered credential and either moves to the passphrase step or connects to a bunker.
fn start_import(secret_input: &Entity<InputState>, state: &Entity<ImportState>, cx: &mut App) {
    let credential = match Credential::parse(secret_input.read(cx).value()) {
        Some(credential) => credential,
        None => {
            state.update(cx, |state, cx| {
                state
                    .progress
                    .fail("Enter an nsec, ncryptsec or bunker:// URI");
                cx.notify();
            });
            return;
        }
    };

    match credential {
        Credential::Nsec(secret) => {
            state.update(cx, |state, cx| {
                state.credential = secret;
                state.stage = Stage::Encrypt;
                state.progress.error = None;
                cx.notify();
            });
        }
        Credential::Ncryptsec(secret) => {
            state.update(cx, |state, cx| {
                state.credential = secret;
                state.stage = Stage::Decrypt;
                state.progress.error = None;
                cx.notify();
            });
        }
        Credential::Bunker(uri) => {
            state.update(cx, |state, cx| {
                state.progress.begin();
                cx.notify();
            });

            let backend = Backend::global(cx);
            let task = backend.update(cx, |backend, cx| backend.import_bunker(&uri, cx));
            finish_import(task, state, cx);
        }
    }
}

/// Encrypts the entered `nsec` with the passphrase and signs in with it.
fn import_nsec(
    pass_input: &Entity<InputState>,
    repass_input: &Entity<InputState>,
    state: &Entity<ImportState>,
    cx: &mut App,
) {
    let backend = Backend::global(cx);
    let nsec = state.read(cx).credential.clone();
    let pass = pass_input.read(cx).value().to_string();
    let repass = repass_input.read(cx).value().to_string();

    if pass.is_empty() {
        state.update(cx, |state, cx| {
            state.progress.fail("Passphrase must not be empty");
            cx.notify();
        });
        return;
    }

    if pass != repass {
        state.update(cx, |state, cx| {
            state.progress.fail("Passphrases do not match");
            cx.notify();
        });
        return;
    }

    state.update(cx, |state, cx| {
        state.progress.begin();
        cx.notify();
    });

    let task = backend.update(cx, |backend, cx| backend.import_nsec(&nsec, &pass, cx));
    finish_import(task, state, cx);
}

/// Decrypts the entered `ncryptsec` with the passphrase and signs in with it.
fn import_ncryptsec(pass_input: &Entity<InputState>, state: &Entity<ImportState>, cx: &mut App) {
    let backend = Backend::global(cx);
    let ncryptsec = state.read(cx).credential.clone();
    let pass = pass_input.read(cx).value().to_string();

    if pass.is_empty() {
        state.update(cx, |state, cx| {
            state.progress.fail("Passphrase must not be empty");
            cx.notify();
        });
        return;
    }

    state.update(cx, |state, cx| {
        state.progress.begin();
        cx.notify();
    });

    let task = backend.update(cx, |backend, cx| {
        backend.import_ncryptsec(&ncryptsec, &pass, cx)
    });
    finish_import(task, state, cx);
}

/// Runs the import task to completion, closing the dialog on success.
fn finish_import<T: 'static>(
    task: Task<Result<T, anyhow::Error>>,
    state: &Entity<ImportState>,
    cx: &mut App,
) {
    let state = state.clone();

    cx.spawn(async move |cx| match task.await {
        Ok(_) => cx.update(close_global),
        Err(error) => {
            state.update(cx, |state, cx| {
                state.progress.fail(error.to_string());
                cx.notify();
            });
        }
    })
    .detach();
}
