pub mod about;
pub mod create;
pub mod grasp_servers;
pub mod init;
pub mod reset;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, WeakEntity, Window, div,
};
use ui::button::{Button, ButtonVariant};
use ui::dialog::{Dialog, DialogHandle};
use ui::theme::ActiveTheme as _;

use crate::repo::RepoDetailView;
use crate::shared::log_dropped;

/// A sync failed; explains why.
pub struct SyncErrorDialog {
    handle: DialogHandle,
    message: SharedString,
}

impl SyncErrorDialog {
    pub fn new(message: SharedString, cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self {
            handle: DialogHandle::new(true),
            message,
        })
    }
}

impl Render for SyncErrorDialog {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let close = self.handle.clone();
        let message = self.message.clone();

        Dialog::new("sync-error-dialog")
            .handle(self.handle.clone())
            .alert(true)
            .title("Sync failed")
            .content(move |_, cx| {
                div()
                    .text_sm()
                    .text_color(cx.theme().colors.muted_foreground)
                    .child(message.clone())
                    .into_any_element()
            })
            .actions(move |_, _| {
                let close = close.clone();
                vec![
                    Button::new("close-sync-error")
                        .label("Close")
                        .variant(ButtonVariant::Secondary)
                        .on_click(move |_, _, cx| close.close(cx))
                        .into_any_element(),
                ]
            })
    }
}

/// Asks before the repository's events are deleted from Nostr.
pub struct DeleteRepositoryDialog {
    handle: DialogHandle,
    view: WeakEntity<RepoDetailView>,
}

impl DeleteRepositoryDialog {
    pub fn new(view: WeakEntity<RepoDetailView>, cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self {
            handle: DialogHandle::new(true),
            view,
        })
    }
}

impl Render for DeleteRepositoryDialog {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        const WARNING: &str = "This removes the repository's announcement, state, and activity from Nostr. This cannot be undone.";

        let cancel_close = self.handle.clone();
        let confirm_close = self.handle.clone();
        let view = self.view.clone();

        Dialog::new("delete-repository-dialog")
            .handle(self.handle.clone())
            .alert(true)
            .title("Delete repository")
            .content(|_, cx| {
                div()
                    .text_sm()
                    .text_color(cx.theme().colors.muted_foreground)
                    .child(WARNING)
                    .into_any_element()
            })
            .actions(move |_, _| {
                let cancel_close = cancel_close.clone();
                let confirm_close = confirm_close.clone();
                let cancel: AnyElement = Button::new("cancel-delete")
                    .label("Cancel")
                    .variant(ButtonVariant::Secondary)
                    .on_click(move |_, _, cx| cancel_close.close(cx))
                    .into_any_element();

                let delete: AnyElement = Button::new("confirm-delete")
                    .label("Delete")
                    .variant(ButtonVariant::Destructive)
                    .on_click({
                        let view = view.clone();
                        move |_, _, cx| {
                            confirm_close.close(cx);
                            log_dropped(view.update(cx, |this, cx| this.delete_repository(cx)));
                        }
                    })
                    .into_any_element();

                vec![cancel, delete]
            })
    }
}
