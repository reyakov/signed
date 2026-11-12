use gpui::{
    AnyElement, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    ScrollHandle, SharedString, Styled as _, Subscription, Window, div,
};
use gpui_native_kit::avatar::Avatar;
use gpui_native_kit::common::{Sizable as _, h_flex, v_flex};
use gpui_native_kit::empty_state::EmptyState;
use gpui_native_kit::input::InputState;
use gpui_native_kit::layout::dock::Panel;
use gpui_native_kit::scroll_area::ScrollArea;
use gpui_native_kit::theme::ActiveTheme as _;
use metadata::ProfileDirectory;
use nostr::prelude::{EventId, PublicKey};
use signed_core::GitEvent;
use signed_state::RepoStore;
use utils::relative_time;

use crate::issues::helpers::{comment_form, comments_section, issue_roots, sidebar_section};
use crate::shared::widgets::{repo_tab_avatar, status_badge};

pub struct IssueDetailView {
    store: Entity<RepoStore>,
    issue_id: EventId,
    comment_input: Entity<InputState>,
    /// Profiles for the author, commenters and the repository owner.
    directory: Entity<ProfileDirectory>,
    scroll_handle: ScrollHandle,
    _subscription: Subscription,
}

impl IssueDetailView {
    pub fn new(store: Entity<RepoStore>, issue_id: EventId, cx: &mut Context<Self>) -> Self {
        let comment_input = InputState::new(cx);
        comment_input.update(cx, |state, cx| state.set_multiline(true, cx));

        let subscription = cx.observe(&store, |this, _store, cx| {
            this.track_participants(cx);
            cx.notify();
        });

        let directory = ProfileDirectory::new(cx);
        let weak = cx.entity().downgrade();
        cx.defer(move |cx| {
            let _ = weak.update(cx, |this, cx| this.track_participants(cx));
        });

        Self {
            store,
            issue_id,
            comment_input,
            directory,
            scroll_handle: ScrollHandle::new(),
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

impl Panel for IssueDetailView {
    fn title(&self, cx: &gpui::App) -> Option<SharedString> {
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

        Some(short_id)
    }

    fn lead(&self, cx: &gpui::App) -> Option<AnyElement> {
        Some(repo_tab_avatar(self.store.read(cx), &self.directory, cx))
    }
}

impl Render for IssueDetailView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);

        let Some(issue) = store.issues.iter().find(|issue| issue.id == self.issue_id) else {
            let message = if store.loaded {
                "Issue not found"
            } else {
                "Loading issue..."
            };
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(EmptyState::new(message))
                .into_any_element();
        };

        let (title, author, status, age, issue_id, content, pubkey) = {
            let profile = self.directory.read(cx).profile(&issue.pubkey, cx);
            let content = if issue.content.is_empty() {
                SharedString::from("No description provided.")
            } else {
                SharedString::from(&issue.content)
            };

            (
                issue.activity_subject(),
                profile.name(),
                store.status_of(issue),
                relative_time(issue.created_at),
                issue.id,
                content,
                issue.pubkey,
            )
        };
        let picture = self.directory.read(cx).profile(&pubkey, cx).picture();

        h_flex()
            .id("issue-detail")
            .size_full()
            .child(
                ScrollArea::new("issue-detail-scroll")
                    .handle(self.scroll_handle.clone())
                    .flex_1()
                    .min_w_0()
                    .child(
                        v_flex()
                            .p_4()
                            .gap_6()
                            .child(
                                h_flex().gap_2().child(status_badge(status)).child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(title),
                                ),
                            )
                            .child(
                                v_flex()
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
                                                                    .picture(picture)
                                                                    .small(),
                                                            )
                                                            .child(author),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_color(
                                                                cx.theme().colors.muted_foreground,
                                                            )
                                                            .child("opened"),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_color(
                                                                cx.theme().colors.muted_foreground,
                                                            )
                                                            .child(SharedString::from(age)),
                                                    ),
                                            )
                                            .child(div().text_sm().child(content)),
                                    )
                                    .child(comments_section(
                                        &self.store,
                                        issue_id,
                                        &self.directory,
                                        cx,
                                    ))
                                    .child(comment_form(
                                        &self.store,
                                        issue_id,
                                        issue_roots,
                                        &self.comment_input,
                                        "comment",
                                        cx,
                                    )),
                            ),
                    ),
            )
            .child(sidebar_section(
                &self.store,
                issue_id,
                issue_roots,
                true,
                &self.directory,
                cx,
            ))
            .into_any_element()
    }
}
