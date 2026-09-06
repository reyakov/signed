use dock::{BasePanel, Panel, PanelEvent};
use gpui::prelude::*;
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString, Window, div,
    relative,
};
use gpui_component::input::TextareaState;
use gpui_component::scroll::ScrollableElement;
use gpui_component::{ActiveTheme, StyledExt, h_flex, v_flex};
use nostr::prelude::EventId;
use signed_core::activity_subject;
use signed_state::{ProfileStore, RepoStore};
use signed_ui::{UserAvatar, placeholder, status_badge};
use utils::relative_time;

use super::helpers::{comment_form, comments_section, issue_roots, sidebar_section};

/// Detail panel of a single issue.
pub struct IssueDetailView {
    /// Repo store holding the issues and their statuses.
    store: Entity<RepoStore>,
    issue_id: EventId,
    /// Input state of the comment textarea.
    comment_input: Entity<TextareaState>,
    focus_handle: FocusHandle,
}

impl IssueDetailView {
    pub fn new(
        store: Entity<RepoStore>,
        issue_id: EventId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let comment_input =
            cx.new(|cx| TextareaState::new(window, cx).placeholder("Leave a comment..."));

        Self {
            focus_handle: cx.focus_handle(),
            store,
            issue_id,
            comment_input,
        }
    }
}

impl BasePanel for IssueDetailView {
    fn panel_name(&self) -> &'static str {
        "issue_detail"
    }
}

impl Panel for IssueDetailView {
    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let short_id = self
            .store
            .read(cx)
            .issues
            .iter()
            .find(|issue| issue.id == self.issue_id)
            .map(|issue| {
                let hex = issue.id.to_hex();
                SharedString::from(&hex[..8])
            })
            .unwrap_or_else(|| SharedString::from("Issue"));

        div().text_sm().child(short_id)
    }
}

impl EventEmitter<PanelEvent> for IssueDetailView {}

impl Focusable for IssueDetailView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for IssueDetailView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);

        let Some(issue) = store.issues.iter().find(|issue| issue.id == self.issue_id) else {
            return placeholder("Issue not found", cx);
        };

        let (title, author, picture, status, age, issue_id, content) = {
            let profile_store = ProfileStore::global(cx);
            let profile = profile_store.read(cx).get(&issue.pubkey);
            let content = if issue.content.is_empty() {
                SharedString::from("No description provided.")
            } else {
                SharedString::from(&issue.content)
            };

            (
                activity_subject(issue),
                profile.name(),
                profile.picture(),
                store.status_of(issue),
                relative_time(issue.created_at),
                issue.id,
                content,
            )
        };

        h_flex()
            .image_cache(gpui::retain_all("issue-detail"))
            .id("issue-detail")
            .size_full()
            .child(
                v_flex()
                    .px_4()
                    .pb_4()
                    .gap_6()
                    .size_full()
                    .min_w_0()
                    .overflow_y_scrollbar()
                    .child(
                        h_flex()
                            .min_h_16()
                            .gap_2()
                            .child(status_badge(status, cx))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .font_semibold()
                                    .line_height(relative(1.2))
                                    .child(title),
                            ),
                    )
                    .child(
                        v_flex()
                            .px_4()
                            .gap_8()
                            .child(
                                v_flex()
                                    .gap_4()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .text_sm()
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .child(
                                                        UserAvatar::new(&author).picture(picture),
                                                    )
                                                    .child(author),
                                            )
                                            .child(
                                                div()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(SharedString::from("opened")),
                                            )
                                            .child(
                                                div()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(SharedString::from(age)),
                                            ),
                                    )
                                    .child(div().text_sm().child(content)),
                            )
                            .child(comments_section(&self.store, issue_id, cx))
                            .child(comment_form(
                                &self.store,
                                issue_id,
                                issue_roots,
                                &self.comment_input,
                                "comment",
                                cx,
                            )),
                    ),
            )
            .child(sidebar_section(
                &self.store,
                issue_id,
                issue_roots,
                false,
                cx,
            ))
            .into_any_element()
    }
}
