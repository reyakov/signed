use gpui::{
    App, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    ScrollHandle, SharedString, Styled as _, Subscription, Window, div,
};
use metadata::ProfileDirectory;
use nostr::prelude::EventId;
use signed_core::GitEvent;
use signed_state::RepoStore;
use ui::avatar::Avatar;
use ui::common::{Sizable as _, StyledExt as _, h_flex, v_flex};
use ui::empty_state::EmptyState;
use ui::input::InputState;
use ui::layout::panel::Panel;
use ui::scroll_area::ScrollArea;
use ui::spinner::Spinner;
use ui::theme::ActiveTheme as _;
use utils::relative_time;

use crate::issues::helpers::{comment_form, comments_section, sidebar_section};
use crate::shared::log_dropped;
use crate::shared::widgets::status_badge;

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
        let comment_input = InputState::builder().multiline().build(cx);
        let directory = ProfileDirectory::new(cx);
        let weak = cx.entity().downgrade();

        let subscription = cx.observe(&store, |this, _store, cx| {
            this.track_participants(cx);
            cx.notify();
        });

        cx.defer(move |cx| {
            log_dropped(weak.update(cx, |this, cx| {
                this.track_participants(cx);
            }));
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
        let author = store
            .issues
            .iter()
            .find(|issue| issue.id == self.issue_id)
            .map(|issue| issue.pubkey);
        let owner = store.announcement.as_ref().map(|a| a.owner);
        let comments: Vec<_> = store
            .comments_of(&self.issue_id)
            .map(|comment| comment.pubkey)
            .collect();
        crate::shared::track_event_participants(
            author,
            owner,
            comments.into_iter(),
            &self.directory,
            cx,
        );
    }
}

impl Panel for IssueDetailView {
    fn title(&self, _cx: &App) -> SharedString {
        let hex = self.issue_id.to_hex();
        let short = hex.get(..8).unwrap_or(&hex);
        SharedString::from(format!("Issue {short}"))
    }

    fn loading(&self, cx: &App) -> bool {
        !self.store.read(cx).loaded
    }
}

impl Render for IssueDetailView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);

        let Some(issue) = store.issues.iter().find(|issue| issue.id == self.issue_id) else {
            // A not-yet-loaded store is not the same as a missing issue.
            return if store.loaded {
                EmptyState::centered("Issue not found")
            } else {
                Spinner::centered("issue-loading")
            };
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
                                        .text_sm()
                                        .font_semibold()
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
                                                    .text_base()
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
                                            .child(div().text_base().child(content)),
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
                                        &store.issues,
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
                &store.issues,
                true,
                &self.directory,
                cx,
            ))
            .into_any_element()
    }
}
