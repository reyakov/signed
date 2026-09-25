use dock::{BasePanel, DockArea, Panel, PanelEvent, add_center_panel, panel_handle};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString,
    WeakEntity, Window, div, px,
};
use gpui_base::{Button as BaseButton, StyledExt};
use gpui_component::input::{Input, InputState, Textarea, TextareaState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::spinner::Spinner;
use gpui_component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use signed_state::RepoStore;

use crate::views::{repo_tab_avatar, tab_title};

pub struct SendPatchView {
    focus_handle: FocusHandle,
    dock_area: WeakEntity<DockArea>,
    store: Entity<RepoStore>,
    repo_name: SharedString,
    subject: Entity<InputState>,
    description: Entity<TextareaState>,
    patch: Entity<TextareaState>,
    submitting: bool,
    /// Error of the last submit attempt, it keeps the panel open.
    error: Option<SharedString>,
}

impl SendPatchView {
    pub fn new(
        dock_area: WeakEntity<DockArea>,
        store: Entity<RepoStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let repo_name = store.read(cx).name();
        let subject = cx.new(|cx| InputState::new(window, cx).placeholder("Title"));

        let description = cx
            .new(|cx| TextareaState::new(window, cx).placeholder("Describe the change (optional)"));

        let patch = cx.new(|cx| {
            TextareaState::new(window, cx).placeholder("diff --git a/file.txt b/file.txt\nindex 1234567..abcdefg 100644\n--- a/file.txt\n+++ b/file.txt")
        });

        Self {
            focus_handle: cx.focus_handle(),
            dock_area,
            store,
            repo_name,
            subject,
            description,
            patch,
            submitting: false,
            error: None,
        }
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.submitting {
            return;
        }

        let subject = self.subject.read(cx).value().to_string();
        let description = self.description.read(cx).value().to_string();
        let patch = self.patch.read(cx).value().to_string();

        if patch.is_empty() {
            return;
        }

        let store = self.store.clone();
        let dock_area = self.dock_area.clone();
        let entity = cx.entity().clone();

        self.submitting = true;
        self.error = None;
        cx.notify();

        // Errors the store detects before publishing.
        let sync_error = store.update(cx, |store, cx| {
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

        // Close the panel once the publish is underway.
        cx.defer_in(window, {
            let dock_area = dock_area.clone();
            let entity = entity.clone();
            move |_, window, cx| {
                if let Some(dock_area) = dock_area.upgrade() {
                    dock_area.update(cx, |dock, cx| {
                        dock.remove_panel(entity, window, cx);
                    });
                }
            }
        });

        cx.notify();
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let can_submit = !self.submitting
            && !self.subject.read(cx).value().is_empty()
            && !self.patch.read(cx).value().is_empty();

        h_flex()
            .px_4()
            .h_16()
            .w_full()
            .gap_2()
            .items_center()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(div().flex_1())
            .child(
                BaseButton::new("send-patch")
                    .h_flex()
                    .h_8()
                    .px_2()
                    .gap_1()
                    .text_sm()
                    .items_center()
                    .justify_center()
                    .bg(cx.theme().primary)
                    .text_color(cx.theme().primary_foreground)
                    .hover(|this| this.bg(cx.theme().primary_hover))
                    .active(|this| this.bg(cx.theme().primary_active))
                    .map(|this| {
                        if self.submitting {
                            this.child(Spinner::new().small())
                        } else {
                            this.child(Icon::new(IconName::ArrowUp)).child("Send patch")
                        }
                    })
                    .disabled(!can_submit)
                    .on_click(cx.listener(|this, _event, window, cx| {
                        this.submit(window, cx);
                    })),
            )
            .into_any_element()
    }

    fn render_inputs(&self, _cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .px_4()
            .py_2()
            .w_full()
            .gap_2()
            .child(Input::new(&self.subject))
            .child(Textarea::new(&self.description).h(px(64.)))
            .into_any_element()
    }

    fn render_patch(&self, cx: &mut Context<Self>) -> AnyElement {
        const MSG: &str = "You can paste a git diff or a git format-patch patch series here.";

        v_flex()
            .px_4()
            .py_2()
            .w_full()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(MSG),
            )
            .child(Textarea::new(&self.patch).h_56())
            .into_any_element()
    }
}

pub(super) fn open_send_patch_panel(
    dock_area: WeakEntity<DockArea>,
    store: Entity<RepoStore>,
    window: &mut Window,
    cx: &mut App,
) {
    let panel = cx.new(|cx| SendPatchView::new(dock_area.clone(), store, window, cx));

    let _ = dock_area.update(cx, |dock_area, cx| {
        add_center_panel(dock_area, panel_handle(panel), window, cx);
    });
}

impl BasePanel for SendPatchView {
    fn panel_name(&self) -> &'static str {
        "send-patch"
    }
}

impl Panel for SendPatchView {
    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let avatar = repo_tab_avatar(self.store.read(cx), cx);
        let label = SharedString::from(format!("{}/send-patch", self.repo_name));

        tab_title(avatar, label)
    }
}

impl EventEmitter<PanelEvent> for SendPatchView {}

impl Focusable for SendPatchView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SendPatchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("send-patch")
            .size_full()
            .child(
                v_flex()
                    .overflow_y_scrollbar()
                    .flex_1()
                    .w_full()
                    .child(self.render_inputs(cx))
                    .when_some(self.error.clone(), |this, error| {
                        this.child(
                            h_flex()
                                .px_4()
                                .py_1()
                                .w_full()
                                .text_xs()
                                .text_color(cx.theme().danger)
                                .child(error),
                        )
                    })
                    .child(self.render_patch(cx)),
            )
            .child(self.render_footer(cx))
    }
}
