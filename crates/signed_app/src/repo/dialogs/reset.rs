use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Task, Window, div, px,
};
use signed_state::RepoStore;
use ui::button::{Button, ButtonVariant};
use ui::common::{ResultLogErr as _, v_flex};
use ui::dialog::{Dialog, DialogHandle};
use ui::theme::ActiveTheme as _;

use crate::shared::log_dropped;

pub struct ResetRemoteDialog {
    handle: DialogHandle,
    store: Entity<RepoStore>,
    deleted: Arc<[SharedString]>,
    /// Servers that could not be listed, so their refs are missing from `deleted`.
    unreachable: Arc<[SharedString]>,
    busy: bool,
    error: Option<SharedString>,
    task: Rc<RefCell<Option<Task<()>>>>,
}

impl ResetRemoteDialog {
    pub fn new(
        store: Entity<RepoStore>,
        deleted: Vec<String>,
        unreachable: Vec<String>,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|_| Self {
            handle: DialogHandle::new(true),
            store,
            deleted: deleted.into_iter().map(SharedString::from).collect(),
            unreachable: unreachable.into_iter().map(SharedString::from).collect(),
            busy: false,
            error: None,
            task: Rc::new(RefCell::new(None)),
        })
    }
}

impl Render for ResetRemoteDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        const WARNING: &str = "Grasp servers are overwritten to match the local mirror. Refs that do not exist locally are deleted from the servers. This cannot be undone.";
        const ERROR: &str = "Could not reach some grasp servers, their refs are not listed:";

        let deleted = Arc::clone(&self.deleted);
        let unreachable = Arc::clone(&self.unreachable);

        let action_unreachable = unreachable.clone();
        let store = self.store.clone();
        let cancel_close = self.handle.clone();
        let confirm_close = self.handle.clone();
        let confirm_task = self.task.clone();
        let busy = self.busy;
        let error = self.error.clone();

        let this = cx.weak_entity();

        Dialog::new("reset-remote-dialog")
            .handle(self.handle.clone())
            .close_on_escape(!busy)
            .close_on_backdrop(!busy)
            .min_width(px(480.))
            .title("Reset remote to local")
            .content(move |_, cx| {
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
                    .map(|this| {
                        if deleted.is_empty() {
                            this.child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().colors.muted_foreground)
                                    .child("No server refs will be deleted."),
                            )
                        } else {
                            this.child(v_flex().gap_1().children(deleted.iter().map(|name| {
                                div()
                                    .text_sm()
                                    .font_family(cx.theme().mono_font)
                                    .child(name.clone())
                            })))
                        }
                    })
                    .when_some(error.clone(), |el, error| {
                        el.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().ramps.destructive.base)
                                .child(error),
                        )
                    })
                    .into_any_element()
            })
            .actions(move |_, _cx| {
                let store = store.clone();
                let cancel_close = cancel_close.clone();
                let confirm_close = confirm_close.clone();
                let confirm_task = confirm_task.clone();
                let action_this = this.clone();

                let confirm = Button::new("confirm-reset-remote")
                    .label("Reset remote")
                    .variant(ButtonVariant::Destructive)
                    .loading(busy)
                    .disabled(busy || !action_unreachable.is_empty())
                    .on_click({
                        let this = action_this.clone();
                        move |_, window, cx| {
                            if busy {
                                return;
                            }

                            log_dropped(this.update(cx, |this, cx| {
                                this.busy = true;
                                this.error = None;
                                cx.notify();
                            }));

                            let task = store.update(cx, |store, cx| store.reset_repository(cx));
                            let close = confirm_close.clone();
                            let handle = window.window_handle();
                            let task_this = this.clone();

                            let spawned = cx.spawn(async move |cx| match task.await {
                                Ok(()) => {
                                    cx.update_window(handle, |_, _, cx| close.close(cx))
                                        .log_err();
                                    task_this
                                        .update(cx, |this, cx| {
                                            this.busy = false;
                                            cx.notify();
                                        })
                                        .log_err();
                                }
                                Err(error) => {
                                    task_this
                                        .update(cx, |this, cx| {
                                            this.busy = false;
                                            this.error = Some(error.to_string().into());
                                            cx.notify();
                                        })
                                        .log_err();
                                }
                            });
                            *confirm_task.borrow_mut() = Some(spawned);
                        }
                    })
                    .into_any_element();

                vec![
                    Button::new("cancel-reset-remote")
                        .label("Cancel")
                        .variant(ButtonVariant::Secondary)
                        .disabled(busy)
                        .on_click(move |_, _, cx| cancel_close.close(cx))
                        .into_any_element(),
                    confirm,
                ]
            })
    }
}
