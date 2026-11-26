use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, WeakEntity, Window, div,
};
use signed_state::RepoStore;
use ui::button::Button;
use ui::common::{ResultLogErr as _, h_flex, v_flex};
use ui::icons::{Icon, IconRole};
use ui::input::{Input, InputState};
use ui::layout::panel::{Panel, PanelSlot};
use ui::theme::ActiveTheme as _;

use crate::app::AppRoot;
use crate::shared::log_dropped;
use crate::shared::progress::error_row;

pub struct SendPatchView {
    workspace: WeakEntity<AppRoot>,
    panel_id: SharedString,
    store: Entity<RepoStore>,
    subject: Entity<InputState>,
    description: Entity<InputState>,
    patch: Entity<InputState>,
    submitting: bool,
    error: Option<SharedString>,
}

impl SendPatchView {
    pub fn new(
        workspace: WeakEntity<AppRoot>,
        panel_id: SharedString,
        store: Entity<RepoStore>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subject = InputState::new(cx);
        let description = InputState::builder().multiline().build(cx);
        let patch = InputState::builder().multiline().build(cx);

        Self {
            workspace,
            panel_id,
            store,
            subject,
            description,
            patch,
            submitting: false,
            error: None,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }

        let subject = self.subject.read(cx).value().to_string();
        let description = self.description.read(cx).value().to_string();

        let patch = self.patch.read(cx).value().to_string();
        if patch.is_empty() {
            return;
        }

        self.submitting = true;
        self.error = None;
        cx.notify();

        let task = self.store.update(cx, |store, cx| {
            store.open_pull_request(
                (!subject.is_empty()).then_some(subject),
                description,
                None,
                patch,
                false,
                None,
                None,
                cx,
            )
        });
        let workspace = self.workspace.clone();
        let panel_id = self.panel_id.clone();

        cx.spawn(async move |this, cx| match task.await {
            Ok(()) => {
                log_dropped(workspace.update(cx, |root, cx| {
                    root.panels().clone().update(cx, |panels, cx| {
                        panels.close(PanelSlot::Split, &panel_id, cx)
                    })
                }));
            }
            Err(error) => {
                this.update(cx, |this, cx| {
                    this.submitting = false;
                    this.error = Some(error.to_string().into());
                    cx.notify();
                })
                .log_err();
            }
        })
        .detach();
    }

    fn render_footer(&self, cx: &Context<Self>) -> AnyElement {
        let can_submit = !self.submitting
            && !self.subject.read(cx).value().is_empty()
            && !self.patch.read(cx).value().is_empty();
        let weak = cx.entity().downgrade();

        h_flex()
            .px_4()
            .py_2()
            .w_full()
            .justify_end()
            .border_t_1()
            .border_color(cx.theme().colors.border)
            .child(
                Button::new("send-patch")
                    .label("Send patch")
                    .icon(Icon::named(IconRole::ArrowUp))
                    .loading(self.submitting)
                    .disabled(!can_submit)
                    .on_click(move |_, _, cx| {
                        log_dropped(weak.update(cx, |this, cx| this.submit(cx)));
                    }),
            )
            .into_any_element()
    }
}

impl Panel for SendPatchView {
    fn title(&self, _cx: &App) -> SharedString {
        SharedString::from("Send patch")
    }

    fn loading(&self, _cx: &App) -> bool {
        self.submitting
    }

    fn toolbar(&self, _cx: &App) -> bool {
        true
    }
}

impl Render for SendPatchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("send-patch")
            .size_full()
            .child(
                v_flex()
                    .p_4()
                    .flex_1()
                    .gap_2()
                    .child(div().pr_16().child(
                        Input::new("patch-subject", self.subject.clone()).placeholder("Title"),
                    ))
                    .child(
                        Input::new("patch-description", self.description.clone())
                            .rows(2)
                            .placeholder("Describe the change"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().colors.muted_foreground)
                            .child(
                                "You can paste a git diff or a git format-patch patch series here.",
                            ),
                    )
                    .child(
                        Input::new("patch-body", self.patch.clone())
                            .rows(12)
                            .placeholder("diff --git a/file.txt b/file.txt"),
                    )
                    .when_some(error_row(&self.error, cx), |this, error| this.child(error)),
            )
            .child(self.render_footer(cx))
    }
}
