use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div,
};
use nostr::prelude::Event;
use signed_state::RepoStore;
use ui::common::v_flex;
use ui::dialog::{Dialog, DialogHandle};
use ui::field::Field;
use ui::input::{Input, InputState};
use ui::theme::ActiveTheme as _;

use crate::shared::progress::{DialogProgress, dialog_submit_button};

pub struct UpdatePrDialog {
    handle: DialogHandle,
    store: Entity<RepoStore>,
    root: Event,
    patch: Entity<InputState>,
    state: Entity<DialogProgress>,
}

impl UpdatePrDialog {
    pub fn new(store: Entity<RepoStore>, root: Event, cx: &mut App) -> Entity<Self> {
        let patch = InputState::builder().multiline().build(cx);
        let state = cx.new(|_| DialogProgress::default());
        cx.new(|_| Self {
            handle: DialogHandle::new(true),
            store,
            root,
            patch,
            state,
        })
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        // The publish runs detached in the store; one submission per dialog.
        if self.state.read(cx).busy {
            return;
        }
        let patch = self.patch.read(cx).value().to_string();
        if patch.trim().is_empty() {
            return;
        }
        self.state.update(cx, |state, _| state.begin());
        self.store.update(cx, |store, cx| {
            store.update_pull_request(&self.root, patch, cx);
        });
        self.handle.close(cx);
    }
}

impl Render for UpdatePrDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let patch = self.patch.clone();
        let this = cx.entity().downgrade();
        let action_state = self.state.clone();

        Dialog::new("update-pr-dialog")
            .handle(self.handle.clone())
            .min_width(gpui::px(520.))
            .title("Update pull request")
            .content(move |_, cx| {
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_base()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child("Publish a new revision with the output of `git format-patch`."),
                    )
                    .child(
                        Field::new("update-patch", "Patch")
                            .child(Input::new("update-patch-input", patch.clone()).rows(6)),
                    )
                    .into_any_element()
            })
            .actions(move |_, cx| {
                let this = this.clone();
                let busy = action_state.read(cx).busy;
                vec![
                    dialog_submit_button(
                        "submit-update",
                        "Update pull request",
                        None,
                        busy,
                        &this,
                        |this, _window, cx| {
                            this.submit(cx);
                            Ok(())
                        },
                    )
                    .into_any_element() as AnyElement,
                ]
            })
    }
}
