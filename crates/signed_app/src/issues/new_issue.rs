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
use signed_state::RepoStore;

use crate::shared::dialogs::{close_global, open_global};

pub struct NewIssueDialog {
    store: Entity<RepoStore>,
    subject: Entity<InputState>,
    content: Entity<InputState>,
}

impl NewIssueDialog {
    pub fn new(store: Entity<RepoStore>, cx: &mut App) -> Entity<Self> {
        let subject = InputState::new(cx);
        let content = InputState::builder().multiline().build(cx);

        cx.new(|_| Self {
            store,
            subject,
            content,
        })
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let subject = self.subject.read(cx).value().to_string();
        let content = self.content.read(cx).value().to_string();
        let subject = (!subject.trim().is_empty()).then_some(subject);

        self.store.update(cx, |store, cx| {
            store.open_issue(subject, content, cx);
        });

        close_global(cx);
    }
}

impl Render for NewIssueDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let subject = self.subject.clone();
        let content = self.content.clone();
        let this = cx.entity().downgrade();

        Dialog::new("new-issue-dialog")
            .open(true)
            .min_width(gpui::px(520.))
            .on_change(|open, _, cx| {
                if !open {
                    close_global(cx);
                }
            })
            .title("New issue")
            .content(move |_, cx| {
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child("Report a bug, ask a question, or propose a change."),
                    )
                    .child(
                        Field::new("issue-title", "Title")
                            .child(Input::new("issue-title-input", subject.clone())),
                    )
                    .child(
                        Field::new("issue-content", "Content")
                            .child(Input::new("issue-content-input", content.clone()).rows(6)),
                    )
                    .into_any_element()
            })
            .actions(move |_, _| {
                let this = this.clone();
                vec![
                    Button::new("create-issue")
                        .label("Create issue")
                        .on_click(move |_, _, cx| {
                            crate::shared::log_dropped(this.update(cx, |this, cx| this.submit(cx)));
                        })
                        .into_any_element() as AnyElement,
                ]
            })
    }
}

/// Opens the new-issue form for `store`.
pub fn open(store: Entity<RepoStore>, cx: &mut App) {
    open_global(NewIssueDialog::new(store, cx), cx);
}
