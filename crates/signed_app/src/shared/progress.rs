use gpui::{
    AnyElement, App, AsyncApp, Context, Entity, IntoElement as _, ParentElement as _, SharedString,
    Styled as _, Subscription, Task, div,
};
use signed_core::RepoAddr;
use signed_state::{Backend, BackendEvent, PushStepStatus};
use ui::common::{h_flex, v_flex};
use ui::dialog::DialogHandle;
use ui::icons::{Icon, IconRole};
use ui::spinner::Spinner;
use ui::theme::ActiveTheme as _;

/// Progress of an async dialog action: a busy flag that disables the form and an error shown below it.
#[derive(Debug, Default)]
pub struct DialogProgress {
    pub busy: bool,
    pub error: Option<SharedString>,
    // The repository whose push checklist is shown; `None` outside a publish.
    push_addr: Option<RepoAddr>,
    pub push_steps: Vec<PushStepStatus>,
    _push_subscription: Option<Subscription>,
}

impl DialogProgress {
    /// Marks an action as started, disabling the form and clearing the previous error.
    pub fn begin(&mut self) {
        self.busy = true;
        self.error = None;
    }

    /// Starts a publish for the repository at `addr` and listens for its push steps.
    pub fn begin_push(&mut self, addr: RepoAddr, cx: &mut Context<Self>) {
        self.begin();
        self.push_addr = Some(addr);
        self.push_steps.clear();

        if self._push_subscription.is_none() {
            let backend = Backend::global(cx);
            self._push_subscription = Some(cx.subscribe(&backend, |this, _backend, event, cx| {
                let BackendEvent::PushProgress { addr, status } = event else {
                    return;
                };

                if this.push_addr.as_ref() != Some(addr) {
                    return;
                }

                match this
                    .push_steps
                    .iter_mut()
                    .find(|step| step.step() == status.step())
                {
                    Some(step) => *step = status.clone(),
                    None => this.push_steps.push(status.clone()),
                }

                cx.notify();
            }));
        }
    }

    /// Marks an action as failed, enabling the form and showing `message`.
    pub fn fail(&mut self, message: impl Into<SharedString>) {
        self.busy = false;
        self.error = Some(message.into());
    }
}

/// The shared error line under a dialog form, `None` when there is no error.
pub fn error_row(error: &Option<SharedString>, cx: &App) -> Option<AnyElement> {
    error.as_ref().map(|message| {
        div()
            .text_base()
            .text_color(cx.theme().ramps.destructive.base)
            .child(message.clone())
            .into_any_element()
    })
}

/// The push checklist of a running publish, `None` before any step is reported.
pub fn push_steps_row(steps: &[PushStepStatus], cx: &App) -> Option<AnyElement> {
    if steps.is_empty() {
        return None;
    }

    let rows = steps
        .iter()
        .map(|status| {
            let label = SharedString::from(status.step().label());

            match status {
                PushStepStatus::Running(_) => h_flex()
                    .gap_2()
                    .text_base()
                    .text_color(cx.theme().colors.muted_foreground)
                    .child(Spinner::new("push-step"))
                    .child(label)
                    .into_any_element(),
                PushStepStatus::Done(_) => h_flex()
                    .gap_2()
                    .text_base()
                    .text_color(cx.theme().colors.foreground)
                    .child(
                        Icon::named(IconRole::Check)
                            .small()
                            .color(cx.theme().colors.foreground),
                    )
                    .child(label)
                    .into_any_element(),
                PushStepStatus::Failed(_, reason) => h_flex()
                    .gap_2()
                    .text_base()
                    .text_color(cx.theme().ramps.destructive.base)
                    .child(
                        Icon::named(IconRole::Close)
                            .small()
                            .color(cx.theme().ramps.destructive.base),
                    )
                    .child(SharedString::from(format!(
                        "{}: {reason}",
                        status.step().label()
                    )))
                    .into_any_element(),
            }
        })
        .collect::<Vec<_>>();

    Some(v_flex().gap_1().children(rows).into_any_element())
}

/// Runs a dialog task to completion.
pub fn run_dialog_task<T: 'static>(
    task: Task<Result<T, anyhow::Error>>,
    state: &Entity<DialogProgress>,
    handle: &DialogHandle,
    cx: &mut App,
) {
    let state = state.clone();
    let handle = handle.clone();

    run_dialog_task_with(
        task,
        cx,
        move |_, cx| cx.update(|cx| handle.close(cx)),
        move |error, cx| {
            state.update(cx, |state, cx| {
                state.fail(error.to_string());
                cx.notify();
            });
        },
    );
}

/// Runs a dialog task to completion, with explicit success and failure sinks.
pub fn run_dialog_task_with<T: 'static>(
    task: Task<Result<T, anyhow::Error>>,
    cx: &mut App,
    on_success: impl FnOnce(T, &mut AsyncApp) + 'static,
    on_error: impl FnOnce(anyhow::Error, &mut AsyncApp) + 'static,
) {
    cx.spawn(async move |cx| match task.await {
        Ok(value) => on_success(value, cx),
        Err(error) => on_error(error, cx),
    })
    .detach();
}
