use dock::{BasePanel, Panel, PanelEvent};
use gpui::prelude::*;
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString, Subscription,
    Window, div, relative,
};
use gpui_component::input::TextareaState;
use gpui_component::scroll::ScrollableElement;
use gpui_component::{ActiveTheme, StyledExt, h_flex, v_flex};
use metadata::ProfileDirectory;
use nostr::prelude::{EventId, PublicKey};
use signed_core::GitEvent;
use signed_state::RepoStore;
use signed_ui::{Avatar, placeholder, status_badge};
use utils::relative_time;

use crate::views::discussion::{comment_form, comments_section, issue_roots, sidebar_section};
use crate::views::{repo_tab_avatar, tab_title};

pub struct IssueDetailView {
    focus_handle: FocusHandle,
    store: Entity<RepoStore>,
    issue_id: EventId,
    comment_input: Entity<TextareaState>,
    /// Profiles for the author, commenters and the repository owner.
    directory: Entity<ProfileDirectory>,
    _subscription: Subscription,
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

        let subscription = cx.observe(&store, |this, _store, cx| {
            this.track_participants(cx);
            cx.notify();
        });

        let directory = ProfileDirectory::new(cx);

        Self {
            focus_handle: cx.focus_handle(),
            store,
            issue_id,
            comment_input,
            directory,
            _subscription: subscription,
        }
    }

    /// Track the issue author, everyone who commented and the repository owner.
    fn track_participants(&mut self, cx: &mut Context<Self>) {
        let store = self.store.read(cx);
        let mut keys: Vec<PublicKey> = store
            .issues
            .iter()
            .find(|issue| issue.id == self.issue_id)
            .map(|issue| issue.pubkey)
            .into_iter()
            .collect();

        keys.extend(
            store
                .comments_of(&self.issue_id)
                .map(|comment| comment.pubkey),
        );

        if let Some(owner) = store.announcement.as_ref().map(|a| a.owner) {
            keys.push(owner);
        }

        self.directory.update(cx, |directory, cx| {
            directory.track(keys, cx);
        });
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

        let avatar = repo_tab_avatar(self.store.read(cx), &self.directory, cx);

        tab_title(avatar, short_id)
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
            // The store has not applied its first pass yet, the issue may still arrive.
            return if store.loaded {
                placeholder("Issue not found", cx)
            } else {
                placeholder("Loading issue...", cx)
            };
        };

        let (title, author, picture, status, age, issue_id, content, pubkey) = {
            let profile = self.directory.read(cx).profile(&issue.pubkey, cx);
            let content = if issue.content.is_empty() {
                SharedString::from("No description provided.")
            } else {
                SharedString::from(&issue.content)
            };

            (
                issue.activity_subject(),
                profile.name(),
                profile.picture(),
                store.status_of(issue),
                relative_time(issue.created_at),
                issue.id,
                content,
                issue.pubkey,
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
                                                        Avatar::new(pubkey)
                                                            .name(author.clone())
                                                            .picture(picture),
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
                            .child(comments_section(&self.store, issue_id, &self.directory, cx))
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
                &self.directory,
                cx,
            ))
            .into_any_element()
    }
}
