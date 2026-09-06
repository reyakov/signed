use gpui::prelude::*;
use gpui::{AnyElement, App, SharedString, div};
use gpui_component::ActiveTheme;

/// Progress of an async dialog action: a busy flag disabling the form,
/// and an error line shown under it.
#[derive(Debug, Default)]
pub struct DialogProgress {
    pub busy: bool,
    pub error: Option<SharedString>,
}

impl DialogProgress {
    /// An action started, disable the form and clear the previous error.
    pub fn begin(&mut self) {
        self.busy = true;
        self.error = None;
    }

    /// An action failed, re-enable the form and surface `message`.
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
