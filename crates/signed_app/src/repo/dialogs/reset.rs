use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div, px,
};
use gpui_native_kit::button::{Button, ButtonVariant};
use gpui_native_kit::common::v_flex;
use gpui_native_kit::dialog::Dialog;
use gpui_native_kit::theme::{ActiveTheme as _, TextRole, Typeset as _};
use signed_state::RepoStore;

use crate::shared::dialogs::{close_global, open_global};

pub struct ResetRemoteDialog {
    store: Entity<RepoStore>,
    deleted: Vec<String>,
}

impl ResetRemoteDialog {
    pub fn new(store: Entity<RepoStore>, deleted: Vec<String>, cx: &mut App) -> Entity<Self> {
        cx.new(|_| Self { store, deleted })
    }
}

impl Render for ResetRemoteDialog {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let deleted = self.deleted.clone();
        let store = self.store.clone();

        Dialog::new("reset-remote-dialog")
            .open(true)
            .min_width(px(480.))
            .on_change(|open, _, cx| {
                if !open {
                    close_global(cx);
                }
            })
            .title("Reset remote to local")
            .content(move |_, cx| {
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .typeset(TextRole::Body, cx)
                            .text_color(cx.theme().colors.muted_foreground)
                            .child(
                                "Grasp servers are overwritten to match the local mirror. Refs that do not exist locally are deleted from the servers. This cannot be undone.",
                            ),
                    )
                    .child(if deleted.is_empty() {
                        div()
                            .typeset(TextRole::Body, cx)
                            .text_color(cx.theme().colors.muted_foreground)
                            .child("No server refs will be deleted.")
                            .into_any_element()
                    } else {
                        v_flex()
                            .gap_1()
                            .children(deleted.iter().map(|name| {
                                div()
                                    .typeset(TextRole::Body, cx)
                                    .font_family(cx.theme().mono_font)
                                    .child(name.clone())
                            }))
                            .into_any_element()
                    })
                    .into_any_element()
            })
            .actions(move |_, _| {
                let store = store.clone();
                let cancel: AnyElement = Button::new("cancel-reset-remote")
                    .label("Cancel")
                    .variant(ButtonVariant::Secondary)
                    .on_click(|_, _, cx| close_global(cx))
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
                        close_global(cx);
                    })
                    .into_any_element();

                vec![cancel, confirm]
            })
    }
}

/// Opens the reset confirmation listing the refs the reset deletes.
pub fn open(store: Entity<RepoStore>, deleted: Vec<String>, cx: &mut App) {
    open_global(ResetRemoteDialog::new(store, deleted, cx), cx);
}
