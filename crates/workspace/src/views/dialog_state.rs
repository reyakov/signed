use gpui::prelude::*;
use gpui::{AnyElement, App, Context, SharedString, Subscription, div};
use gpui_component::spinner::Spinner;
use gpui_component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use signed_core::RepoAddr;
use signed_state::{Backend, BackendEvent, PushStepStatus};

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
            .text_sm()
            .text_color(cx.theme().danger)
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
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(Spinner::new().small())
                    .child(label)
                    .into_any_element(),
                PushStepStatus::Done(_) => h_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(Icon::new(IconName::CircleCheck).small())
                    .child(label)
                    .into_any_element(),
                PushStepStatus::Failed(_, reason) => h_flex()
                    .gap_2()
                    .text_sm()
                    .text_color(cx.theme().danger)
                    .child(Icon::new(IconName::Close).small())
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
