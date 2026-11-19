use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div, px,
};
use signed_state::RepoStore;
use ui::button::{Button, ButtonVariant};
use ui::common::v_flex;
use ui::dialog::{Dialog, DialogHandle};
use ui::theme::ActiveTheme as _;

pub struct ResetRemoteDialog {
    handle: DialogHandle,
    store: Entity<RepoStore>,
    deleted: Vec<String>,
}

impl ResetRemoteDialog {
    pub fn new(store: Entity<RepoStore>, deleted: Vec<String>, cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self {
            handle: DialogHandle::new(true),
            store,
            deleted,
        })
    }
}

impl Render for ResetRemoteDialog {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let deleted = self.deleted.clone();
        let store = self.store.clone();
        let cancel_close = self.handle.clone();
        let confirm_close = self.handle.clone();

        Dialog::new("reset-remote-dialog")
            .handle(self.handle.clone())
            .min_width(px(480.))
            .title("Reset remote to local")
            .content(move |_, cx| {
                v_flex()
                    .gap_2()
                    .child(
                        div().text_base()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child(
                                "Grasp servers are overwritten to match the local mirror. Refs that do not exist locally are deleted from the servers. This cannot be undone.",
                            ),
                    )
                    .child(if deleted.is_empty() {
                        div().text_base()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child("No server refs will be deleted.")
                            .into_any_element()
                    } else {
                        v_flex()
                            .gap_1()
                            .children(deleted.iter().map(|name| {
                                div().text_base()
                                    .font_family(cx.theme().mono_font)
                                    .child(name.clone())
                            }))
                            .into_any_element()
                    })
                    .into_any_element()
            })
            .actions(move |_, _| {
                let store = store.clone();
                let cancel_close = cancel_close.clone();
                let confirm_close = confirm_close.clone();

                let cancel: AnyElement = Button::new("cancel-reset-remote")
                    .label("Cancel")
                    .variant(ButtonVariant::Secondary)
                    .on_click(move |_, _, cx| cancel_close.close(cx))
                    .into_any_element();

                let confirm: AnyElement = Button::new("confirm-reset-remote")
                    .label("Reset remote")
                    .variant(ButtonVariant::Destructive)
                    .on_click(move |_, _, cx| {
                        let task = store.update(cx, |store, cx| store.reset_repository(cx));
                        cx.spawn(async move |_cx| {
                            if let Err(error) = task.await {
                                log::error!("failed to reset remote: {error:#}");
                            }
                        })
                        .detach();
                        confirm_close.close(cx);
                    })
                    .into_any_element();

                vec![cancel, confirm]
            })
    }
}
