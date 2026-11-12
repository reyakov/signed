use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div,
};
use gpui_native_kit::button::Button;
use gpui_native_kit::common::v_flex;
use gpui_native_kit::dialog::Dialog;
use gpui_native_kit::field::Field;
use gpui_native_kit::input::{Input, InputState};
use gpui_native_kit::theme::ActiveTheme as _;
use nostr::prelude::Event;
use signed_state::RepoStore;

use crate::shared::dialogs::{close_global, open_global};

pub fn open(store: Entity<RepoStore>, root: Event, cx: &mut App) {
    open_global(UpdatePrDialog::new(store, root, cx), cx);
}

pub struct UpdatePrDialog {
    store: Entity<RepoStore>,
    root: Event,
    patch: Entity<InputState>,
}

impl UpdatePrDialog {
    pub fn new(store: Entity<RepoStore>, root: Event, cx: &mut App) -> Entity<Self> {
        let patch = InputState::new(cx);
        patch.update(cx, |state, cx| state.set_multiline(true, cx));
        cx.new(|_| Self { store, root, patch })
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let patch = self.patch.read(cx).value().to_string();
        if patch.trim().is_empty() {
            return;
        }
        self.store.update(cx, |store, cx| {
            store.update_pull_request(&self.root, patch, cx);
        });
        close_global(cx);
    }
}

impl Render for UpdatePrDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let patch = self.patch.clone();
        let this = cx.entity().downgrade();

        Dialog::new("update-pr-dialog")
            .open(true)
            .min_width(gpui::px(520.))
            .on_change(|open, _, cx| {
                if !open {
                    close_global(cx);
                }
            })
            .title("Update pull request")
            .content(move |_, cx| {
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child("Publish a new revision with the output of `git format-patch`."),
                    )
                    .child(
                        Field::new("update-patch", "Patch")
                            .child(Input::new("update-patch-input", patch.clone()).rows(6)),
                    )
                    .into_any_element()
            })
            .actions(move |_, _| {
                let this = this.clone();
                vec![
                    Button::new("submit-update")
                        .label("Update pull request")
                        .on_click(move |_, _, cx| {
                            let _ = this.update(cx, |this, cx| this.submit(cx));
                        })
                        .into_any_element() as AnyElement,
                ]
            })
    }
}
