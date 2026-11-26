use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Task, WeakEntity, Window, div,
};
use signed_state::RepoStore;
use ui::button::Button;
use ui::common::{h_flex, v_flex};
use ui::field::Field;
use ui::input::{Input, InputState};
use ui::layout::panel::{Panel, PanelSlot};
use ui::theme::ActiveTheme as _;

use crate::app::AppRoot;
use crate::shared::log_dropped;
use crate::shared::progress::error_row;

pub struct NewIssueView {
    workspace: WeakEntity<AppRoot>,
    panel_id: SharedString,
    store: Entity<RepoStore>,
    subject: Entity<InputState>,
    content: Entity<InputState>,
    submitting: bool,
    error: Option<SharedString>,
    /// The running publish, cancelled when the view closes.
    submit_task: Option<Task<()>>,
}

impl NewIssueView {
    pub fn new(
        workspace: WeakEntity<AppRoot>,
        panel_id: SharedString,
        store: Entity<RepoStore>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subject = InputState::new(cx);
        let content = InputState::builder().multiline().build(cx);

        Self {
            workspace,
            panel_id,
            store,
            subject,
            content,
            submitting: false,
            error: None,
            submit_task: None,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }

        let subject = self.subject.read(cx).value().to_string();
        let content = self.content.read(cx).value().to_string();
        let subject = (!subject.trim().is_empty()).then_some(subject);

        self.submitting = true;
        self.error = None;
        cx.notify();

        let workspace = self.workspace.clone();
        let panel_id = self.panel_id.clone();
        let task = self
            .store
            .update(cx, |store, cx| store.open_issue(subject, content, cx));
        self.submit_task = Some(cx.spawn(async move |this, cx| match task.await {
            Ok(()) => {
                log_dropped(workspace.update(cx, |root, cx| {
                    root.panels().clone().update(cx, |panels, cx| {
                        panels.close(PanelSlot::Split, &panel_id, cx)
                    })
                }));
            }
            Err(error) => {
                log_dropped(this.update(cx, |this, cx| {
                    this.submitting = false;
                    this.error = Some(error.to_string().into());
                    cx.notify();
                }));
            }
        }));
    }

    fn render_footer(&self, cx: &Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();

        h_flex()
            .px_4()
            .py_2()
            .w_full()
            .justify_end()
            .border_t_1()
            .border_color(cx.theme().colors.border)
            .child(
                Button::new("create-issue")
                    .label("Create issue")
                    .loading(self.submitting)
                    .disabled(self.submitting)
                    .on_click(move |_, _, cx| {
                        log_dropped(weak.update(cx, |this, cx| this.submit(cx)));
                    }),
            )
            .into_any_element()
    }
}

impl Panel for NewIssueView {
    fn title(&self, _cx: &App) -> SharedString {
        SharedString::from("New issue")
    }

    fn loading(&self, _cx: &App) -> bool {
        self.submitting
    }

    fn toolbar(&self, _cx: &App) -> bool {
        true
    }
}

impl Render for NewIssueView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("new-issue")
            .size_full()
            .child(
                v_flex()
                    .p_4()
                    .flex_1()
                    .gap_3()
                    .child(
                        div()
                            .pr_16()
                            .text_base()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child("Report a bug, ask a question, or propose a change."),
                    )
                    .child(
                        Field::new("issue-title", "Title")
                            .child(Input::new("issue-title-input", self.subject.clone())),
                    )
                    .child(
                        Field::new("issue-content", "Content")
                            .child(Input::new("issue-content-input", self.content.clone()).rows(6)),
                    )
                    .when_some(error_row(&self.error, cx), |this, error| this.child(error)),
            )
            .child(self.render_footer(cx))
    }
}
