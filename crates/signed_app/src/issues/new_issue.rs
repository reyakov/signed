use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Window, div,
};
use signed_state::RepoStore;
use ui::button::Button;
use ui::common::v_flex;
use ui::dialog::{Dialog, DialogHandle};
use ui::field::Field;
use ui::input::{Input, InputState};
use ui::theme::{ActiveTheme as _, TextRole, Typeset as _};

pub struct NewIssueDialog {
    handle: DialogHandle,
    store: Entity<RepoStore>,
    subject: Entity<InputState>,
    content: Entity<InputState>,
}

impl NewIssueDialog {
    pub fn new(store: Entity<RepoStore>, cx: &mut App) -> Entity<Self> {
        let subject = InputState::new(cx);
        let content = InputState::builder().multiline().build(cx);

        cx.new(|_| Self {
            handle: DialogHandle::new(true),
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

        self.handle.close(cx);
    }
}

impl Render for NewIssueDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let subject = self.subject.clone();
        let content = self.content.clone();
        let this = cx.entity().downgrade();

        Dialog::new("new-issue-dialog")
            .handle(self.handle.clone())
            .min_width(gpui::px(520.))
            .title("New issue")
            .content(move |_, cx| {
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .typeset(TextRole::Body, cx)
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
