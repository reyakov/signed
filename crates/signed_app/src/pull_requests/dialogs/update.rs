use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Window, div,
};
use nostr::prelude::Event;
use signed_state::RepoStore;
use ui::button::Button;
use ui::common::v_flex;
use ui::dialog::{Dialog, DialogHandle};
use ui::field::Field;
use ui::input::{Input, InputState};
use ui::theme::ActiveTheme as _;

use crate::shared::log_dropped;

pub struct UpdatePrDialog {
    handle: DialogHandle,
    store: Entity<RepoStore>,
    root: Event,
    patch: Entity<InputState>,
    busy: bool,
}

impl UpdatePrDialog {
    pub fn new(store: Entity<RepoStore>, root: Event, cx: &mut App) -> Entity<Self> {
        let patch = InputState::builder().multiline().build(cx);

        cx.new(|_| Self {
            handle: DialogHandle::new(true),
            store,
            root,
            patch,
            busy: false,
        })
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        // The publish runs detached in the store; one submission per dialog.
        if self.busy {
            return;
        }

        let patch = self.patch.read(cx).value().to_string();
        if patch.trim().is_empty() {
            return;
        }

        self.busy = true;
        self.store.update(cx, |store, cx| {
            store.update_pull_request(&self.root, patch, cx);
        });
        self.handle.close(cx);
    }
}

impl Render for UpdatePrDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        const PATCH_HINT: &str = "Publish a new revision with the output of `git format-patch`.";

        let patch = self.patch.clone();
        let this = cx.weak_entity();
        let busy = self.busy;

        Dialog::new("update-pr-dialog")
            .handle(self.handle.clone())
            .min_width(gpui::px(520.))
            .title("Update pull request")
            .content(move |_, cx| {
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child(PATCH_HINT),
                    )
                    .child(
                        Field::new("update-patch", "Patch")
                            .child(Input::new("update-patch-input", patch.clone()).rows(6)),
                    )
                    .into_any_element()
            })
            .actions(move |_, _cx| {
                let this = this.clone();
                vec![
                    Button::new("submit-update")
                        .label("Update pull request")
                        .loading(busy)
                        .disabled(busy)
                        .on_click(move |_, _, cx| {
                            log_dropped(this.update(cx, |this, cx| this.submit(cx)));
                        })
                        .into_any_element(),
                ]
            })
    }
}
