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
use ui::input::{Input, InputState, InputStateEvent};
use ui::theme::ActiveTheme as _;

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
    busy: bool,
    error: Option<SharedString>,
    task: Option<Task<()>>,
}

pub struct ImportDialog {
    handle: DialogHandle,
    secret_input: Entity<InputState>,
    pass_input: Entity<InputState>,
    repass_input: Entity<InputState>,
    state: Entity<ImportState>,
    _subscriptions: Vec<Subscription>,
}

impl ImportDialog {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let handle = DialogHandle::new(true);
        let state = cx.new(|_| ImportState::default());
        let secret_input = InputState::new(cx);
        let pass_input = InputState::builder().password().build(cx);
        let repass_input = InputState::builder().password().build(cx);

        cx.new(|cx| {
            let mut subscriptions = Vec::new();

            for input in [&secret_input, &pass_input, &repass_input] {
                let secret_input = secret_input.clone();
                let pass_input = pass_input.clone();
                let repass_input = repass_input.clone();
                let state = state.clone();
                let handle = handle.clone();

                subscriptions.push(cx.subscribe(input, move |_this, _input, event, cx| {
                    if matches!(event, InputStateEvent::Submitted) {
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

            Self {
                handle,
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.clone();
        let busy = state.read(cx).busy;

        let secret_input = self.secret_input.clone();
        let pass_input = self.pass_input.clone();
        let repass_input = self.repass_input.clone();

        let action_state = state.clone();
        let action_secret = secret_input.clone();

        let action_pass = pass_input.clone();
        let action_repass = repass_input.clone();
        let action_close = self.handle.clone();

        Dialog::new("import-dialog")
            .handle(self.handle.clone())
            .close_on_escape(!busy)
            .close_on_backdrop(!busy)
            .min_width(px(520.))
            .title("Import identity")
            .content(move |_window, cx| {
                let state = state.read(cx);
                let stage = state.stage;

                let description = match stage {
                    Stage::Credential => "Paste the secret key or bunker of an existing identity.",
                    Stage::Encrypt => "Choose a passphrase to encrypt your key on this device.",
                    Stage::Decrypt => "Enter the passphrase used to encrypt this key.",
                };

                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child(description),
                    )
                    .child(match stage {
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
                    })
                    .when_some(state.error.clone(), |el, error| {
                        el.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().ramps.destructive.base)
                                .child(error),
                        )
                    })
                    .into_any_element()
            })
            .actions(move |_, cx| {
                let state = action_state.read(cx);
                let busy = state.busy;

                let label = match state.stage {
                    Stage::Credential => "Continue",
                    Stage::Encrypt => "Import identity",
                    Stage::Decrypt => "Unlock",
                };

                let secret = action_secret.clone();
                let pass = action_pass.clone();
                let repass = action_repass.clone();
                let state = action_state.clone();
                let handle = action_close.clone();

                vec![
                    Button::new("import-identity")
                        .label(label)
                        .loading(busy)
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            submit(&secret, &pass, &repass, &state, &handle, cx);
                        })
                        .into_any_element(),
                ]
            })
    }
}

fn submit(
    secret_input: &Entity<InputState>,
    pass_input: &Entity<InputState>,
    repass_input: &Entity<InputState>,
    state: &Entity<ImportState>,
    handle: &DialogHandle,
    cx: &mut App,
) {
    if state.read(cx).busy {
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
    handle: &DialogHandle,
    cx: &mut App,
) {
    let credential = match Credential::parse(secret_input.read(cx).value()) {
        Some(credential) => credential,
        None => {
            state.update(cx, |state, cx| {
                state.error = Some("Enter an nsec, ncryptsec or bunker:// URI".into());
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
                state.error = None;
                cx.notify();
            });
        }
        Credential::Ncryptsec(secret) => {
            state.update(cx, |state, cx| {
                state.credential = secret;
                state.stage = Stage::Decrypt;
                state.error = None;
                cx.notify();
            });
        }
        Credential::Bunker(uri) => {
            state.update(cx, |state, cx| {
                state.busy = true;
                state.error = None;
                cx.notify();
            });
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
    handle: &DialogHandle,
    cx: &mut App,
) {
    let backend = Backend::global(cx);
    let nsec = state.read(cx).credential.clone();
    let pass = pass_input.read(cx).value().to_string();
    let repass = repass_input.read(cx).value().to_string();

    if pass.is_empty() {
        state.update(cx, |state, cx| {
            state.error = Some("Passphrase must not be empty".into());
            cx.notify();
        });
        return;
    }

    if pass != repass {
        state.update(cx, |state, cx| {
            state.error = Some("Passphrases do not match".into());
            cx.notify();
        });
        return;
    }

    state.update(cx, |state, cx| {
        state.busy = true;
        state.error = None;
        cx.notify();
    });

    let task = backend.update(cx, |backend, cx| backend.import_nsec(&nsec, &pass, cx));
    finish_import(task, state, handle, cx);
}

/// Decrypts the entered `ncryptsec` with the passphrase and signs in with it.
fn import_ncryptsec(
    pass_input: &Entity<InputState>,
    state: &Entity<ImportState>,
    handle: &DialogHandle,
    cx: &mut App,
) {
    let backend = Backend::global(cx);
    let ncryptsec = state.read(cx).credential.clone();
    let pass = pass_input.read(cx).value().to_string();

    if pass.is_empty() {
        state.update(cx, |state, cx| {
            state.error = Some("Passphrase must not be empty".into());
            cx.notify();
        });
        return;
    }

    state.update(cx, |state, cx| {
        state.busy = true;
        state.error = None;
        cx.notify();
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
    handle: &DialogHandle,
    cx: &mut App,
) {
    let handle = handle.clone();
    let weak = state.clone();

    let task = cx.spawn(async move |cx| match task.await {
        Ok(_) => cx.update(|cx| handle.close(cx)),
        Err(error) => {
            weak.update(cx, |state, cx| {
                state.busy = false;
                state.error = Some(error.to_string().into());
                cx.notify();
            });
        }
    });

    state.update(cx, |state, _| {
        state.task = Some(task);
    });
}
