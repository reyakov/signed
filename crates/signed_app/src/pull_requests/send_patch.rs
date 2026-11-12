use assets::CustomIconName;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, WeakEntity, Window, div,
};
use gpui_native_kit::button::Button;
use gpui_native_kit::common::{h_flex, v_flex};
use gpui_native_kit::icons::Icon;
use gpui_native_kit::input::{Input, InputState};
use gpui_native_kit::layout::dock::{DockState, Panel};
use gpui_native_kit::theme::ActiveTheme as _;
use metadata::ProfileDirectory;
use signed_state::RepoStore;

use crate::shared::widgets::repo_tab_avatar;

pub struct SendPatchView {
    dock: WeakEntity<DockState>,
    panel_id: SharedString,
    store: Entity<RepoStore>,
    repo_name: SharedString,
    subject: Entity<InputState>,
    description: Entity<InputState>,
    patch: Entity<InputState>,
    submitting: bool,
    error: Option<SharedString>,
    directory: Entity<ProfileDirectory>,
}

impl SendPatchView {
    pub fn new(
        dock: WeakEntity<DockState>,
        panel_id: SharedString,
        store: Entity<RepoStore>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let repo_name = store.read(cx).name();
        let directory = ProfileDirectory::new(cx);

        if let Some(owner) = store.read(cx).announcement.as_ref().map(|a| a.owner) {
            directory.update(cx, |directory, cx| directory.track([owner], cx));
        }

        let subject = InputState::new(cx);

        let description = InputState::new(cx);
        description.update(cx, |state, cx| state.set_multiline(true, cx));

        let patch = InputState::new(cx);
        patch.update(cx, |state, cx| state.set_multiline(true, cx));

        Self {
            dock,
            panel_id,
            store,
            repo_name,
            subject,
            description,
            patch,
            submitting: false,
            error: None,
            directory,
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

        let sync_error = self.store.update(cx, |store, cx| {
            store.open_pull_request(
                (!subject.is_empty()).then_some(subject),
                description,
                None,
                patch,
                false,
                None,
                None,
                cx,
            );
            store.last_error.clone()
        });

        if let Some(error) = sync_error {
            self.submitting = false;
            self.error = Some(error.into());
            cx.notify();
            return;
        }

        let panel_id = self.panel_id.clone();
        let _ = self
            .dock
            .update(cx, |dock, cx| dock.close_panel(&panel_id, cx));
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
                    .icon(Icon::named(CustomIconName::ArrowUp))
                    .loading(self.submitting)
                    .disabled(!can_submit)
                    .on_click(move |_, _, cx| {
                        let _ = weak.update(cx, |this, cx| this.submit(cx));
                    }),
            )
            .into_any_element()
    }
}

impl Panel for SendPatchView {
    fn title(&self, _cx: &App) -> Option<SharedString> {
        Some(SharedString::from(format!("{}/send-patch", self.repo_name)))
    }

    fn lead(&self, cx: &App) -> Option<AnyElement> {
        Some(repo_tab_avatar(self.store.read(cx), &self.directory, cx))
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
                    .child(Input::new("patch-subject", self.subject.clone()).placeholder("Title"))
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
                    .when_some(self.error.clone(), |this, error| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().ramps.destructive.base)
                                .child(error),
                        )
                    }),
            )
            .child(self.render_footer(cx))
    }
}
