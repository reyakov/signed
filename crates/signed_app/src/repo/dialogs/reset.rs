use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Window, div, px,
};
use signed_state::RepoStore;
use ui::button::{Button, ButtonVariant};
use ui::common::{ResultLogErr as _, v_flex};
use ui::dialog::{Dialog, DialogHandle};
use ui::theme::ActiveTheme as _;

use crate::shared::progress::{DialogProgress, error_row};

pub struct ResetRemoteDialog {
    handle: DialogHandle,
    store: Entity<RepoStore>,
    deleted: Vec<String>,
    /// Servers that could not be listed, so their refs are missing from `deleted`.
    unreachable: Vec<String>,
    state: Entity<DialogProgress>,
}

impl ResetRemoteDialog {
    pub fn new(
        store: Entity<RepoStore>,
        deleted: Vec<String>,
        unreachable: Vec<String>,
        cx: &mut App,
    ) -> Entity<Self> {
        let state = cx.new(|_| DialogProgress::default());
        cx.new(|_| Self {
            handle: DialogHandle::new(true),
            store,
            deleted,
            unreachable,
            state,
        })
    }
}

impl Render for ResetRemoteDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        const WARNING: &str = "Grasp servers are overwritten to match the local mirror. Refs that do not exist locally are deleted from the servers. This cannot be undone.";
        const ERROR: &str = "Could not reach some grasp servers, their refs are not listed:";

        let deleted = self.deleted.clone();
        let unreachable = self.unreachable.clone();
        let action_unreachable = unreachable.clone();
        let store = self.store.clone();
        let cancel_close = self.handle.clone();
        let confirm_close = self.handle.clone();
        let content_state = self.state.clone();
        let action_state = self.state.clone();
        let busy = self.state.read(cx).busy;

        Dialog::new("reset-remote-dialog")
            .handle(self.handle.clone())
            .close_on_escape(!busy)
            .close_on_backdrop(!busy)
            .min_width(px(480.))
            .title("Reset remote to local")
            .content(move |_, cx| {
                let progress = content_state.read(cx);

                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child(WARNING),
                    )
                    .when(!unreachable.is_empty(), |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().ramps.destructive.base)
                                .child(ERROR),
                        )
                        .children(unreachable.iter().map(|url| {
                            div()
                                .text_sm()
                                .font_family(cx.theme().mono_font)
                                .child(url.clone())
                        }))
                    })
                    .child(if deleted.is_empty() {
                        div()
                            .text_sm()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child("No server refs will be deleted.")
                            .into_any_element()
                    } else {
                        v_flex()
                            .gap_1()
                            .children(deleted.iter().map(|name| {
                                div()
                                    .text_sm()
                                    .font_family(cx.theme().mono_font)
                                    .child(name.clone())
                            }))
                            .into_any_element()
                    })
                    .when_some(error_row(&progress.error, cx), |el, error| el.child(error))
                    .into_any_element()
            })
            .actions(move |_, cx| {
                let busy = action_state.read(cx).busy;
                let store = store.clone();
                let cancel_close = cancel_close.clone();
                let confirm_close = confirm_close.clone();
                let confirm_state = action_state.clone();

                let cancel = Button::new("cancel-reset-remote")
                    .label("Cancel")
                    .variant(ButtonVariant::Secondary)
                    .disabled(busy)
                    .on_click(move |_, _, cx| cancel_close.close(cx))
                    .into_any_element();

                let confirm = Button::new("confirm-reset-remote")
                    .label("Reset remote")
                    .variant(ButtonVariant::Destructive)
                    .loading(busy)
                    .disabled(busy || !action_unreachable.is_empty())
                    .on_click(move |_, window, cx| {
                        let task = store.update(cx, |store, cx| store.reset_repository(cx));
                        let state = confirm_state.clone();
                        let close = confirm_close.clone();
                        let handle = window.window_handle();

                        cx.spawn(async move |cx| match task.await {
                            Ok(()) => {
                                cx.update_window(handle, |_, _, cx| close.close(cx))
                                    .log_err();
                            }
                            Err(error) => {
                                state.update(cx, |state, cx| {
                                    state.fail(error.to_string());
                                    cx.notify();
                                });
                            }
                        })
                        .detach();
                    })
                    .into_any_element();

                vec![cancel, confirm]
            })
    }
}
