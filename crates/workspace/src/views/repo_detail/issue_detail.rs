use std::collections::HashMap;

use assets::CustomIconName;
use dock::{BasePanel, Panel, PanelEvent};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Render, SharedString,
    Window, div, px, relative,
};
use gpui_component::avatar::Avatar;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Textarea, TextareaState};
use gpui_component::scroll::ScrollableElement;
use gpui_component::tag::Tag;
use gpui_component::{ActiveTheme, Icon, Sizable, StyledExt, h_flex, v_flex};
use nostr::prelude::{Event, EventId, PublicKey};
use signed_core::activity_subject;
use signed_state::{ProfileStore, RepoStore};
use utils::relative_time;

use super::helpers::{placeholder, status_badge};
use crate::image_cache::{MAX_IMAGES, image_cache};

/// Detail panel of a single issue.
pub struct IssueDetailView {
    focus_handle: FocusHandle,
    /// Repo store holding the issues and their statuses.
    store: Entity<RepoStore>,
    issue_id: EventId,
    /// Input state of the "leave a comment" textarea.
    comment_input: Entity<TextareaState>,
    /// Issue/comment bodies as shared strings, keyed by event ID, so
    /// re-renders don't clone full contents again (events are immutable,
    /// so the cache never needs invalidation).
    contents: HashMap<EventId, SharedString>,
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
            contents: HashMap::new(),
        }
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let profile_store = ProfileStore::global(cx);
        let store = self.store.read(cx);

        let Some(issue) = store.issues.iter().find(|issue| issue.id == self.issue_id) else {
            // `render` already bails out when the issue is missing.
            return div().into_any_element();
        };

        // Participants: the issue author plus everyone who commented.
        let mut participants: Vec<PublicKey> = vec![issue.pubkey];
        participants.extend(store.comments_of(&issue.id).map(|comment| comment.pubkey));
        participants.sort_by_key(PublicKey::to_hex);
        participants.dedup();

        // Issue labels are NIP-34 `t` hashtag tags on the event.
        let labels: Vec<String> = issue.tags.hashtags().map(|tag| tag.to_string()).collect();

        v_flex()
            .w(px(240.))
            .h_full()
            .flex_none()
            .px_4()
            .gap_4()
            .border_l(px(1.))
            .border_color(cx.theme().sidebar_border)
            .child(
                v_flex()
                    .gap_2()
                    .child(sidebar_title("Participants", cx))
                    .children(participants.iter().map(|pubkey| {
                        let profile = profile_store.read(cx).get(pubkey);
                        let name = profile.name();
                        let picture = profile.picture();

                        h_flex()
                            .gap_1()
                            .items_center()
                            .child(
                                Avatar::new()
                                    .name(name.clone())
                                    .when_some(picture, |this, url| this.src(url))
                                    .rounded(cx.theme().radius)
                                    .small(),
                            )
                            .child(div().text_sm().truncate().text_ellipsis().child(name))
                            .into_any_element()
                    })),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(sidebar_title("Labels", cx))
                    .map(|this| {
                        if labels.is_empty() {
                            this.child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("None yet."),
                            )
                        } else {
                            this.child(h_flex().gap_1().children({
                                let mut items = vec![];

                                for label in labels.iter() {
                                    items.push(
                                        Tag::secondary()
                                            .outline()
                                            .xsmall()
                                            .child(SharedString::from(label)),
                                    );
                                }

                                items
                            }))
                        }
                    }),
            )
            .into_any_element()
    }

    fn render_comments(&mut self, id: &EventId, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let comments: Vec<&Event> = store.comments_of(id).collect();
        let title = SharedString::from(format!("Discussions {}", comments.len()));

        v_flex()
            .gap_4()
            .child(div().text_xs().font_semibold().child(title))
            .children(comments.iter().map(|comment| {
                let profile = ProfileStore::global(cx).read(cx).get(&comment.pubkey);
                let author = profile.name();
                let picture = profile.picture();
                let age = relative_time(comment.created_at);
                // Comment bodies are cloned into shared strings once per
                // comment, not on every render.
                let content = self
                    .contents
                    .entry(comment.id)
                    .or_insert_with(|| SharedString::from(comment.content.clone()))
                    .clone();

                v_flex()
                    .gap_1()
                    .p_3()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .child(
                        h_flex()
                            .gap_2()
                            .text_sm()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Avatar::new()
                                            .name(author.clone())
                                            .when_some(picture, |this, url| this.src(url))
                                            .rounded(cx.theme().radius)
                                            .small(),
                                    )
                                    .child(author),
                            )
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("commented"),
                            )
                            .child(
                                div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(age)),
                            ),
                    )
                    .child(div().text_sm().child(content))
            }))
            .into_any_element()
    }

    fn render_form(&mut self, id: &EventId, cx: &mut Context<Self>) -> impl IntoElement {
        let comment_input = self.comment_input.clone();
        let store = self.store.clone();
        let id = id.to_owned();

        v_flex()
            .gap_2()
            .child(
                Textarea::new(&self.comment_input)
                    .h_24()
                    .text_color(cx.theme().muted_foreground)
                    .bg(cx.theme().muted),
            )
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_1()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(Icon::new(CustomIconName::Markdown).small())
                            .child("Markdown is supported"),
                    )
                    .child(
                        Button::new("comment")
                            .primary()
                            .label("Comment")
                            .tooltip("Post comment")
                            .on_click(move |_event, window, cx| {
                                let content = comment_input.read(cx).value().trim().to_string();
                                if content.is_empty() {
                                    return;
                                }
                                let Some(root) = store
                                    .read(cx)
                                    .issues
                                    .iter()
                                    .find(|issue| issue.id == id)
                                    .cloned()
                                else {
                                    return;
                                };
                                store.update(cx, |store, cx| {
                                    store.comment(&root, content, cx);
                                });
                                comment_input.update(cx, |input, cx| {
                                    input.set_value("", window, cx);
                                });
                            }),
                    ),
            )
            .into_any_element()
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
            let content = self
                .contents
                .entry(issue.id)
                .or_insert_with(|| SharedString::from(issue.content.clone()))
                .clone();

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
            .image_cache(image_cache("issue-detail", MAX_IMAGES))
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
                                                        Avatar::new()
                                                            .when_some(picture, |this, url| {
                                                                this.src(url)
                                                            })
                                                            .name(author.clone())
                                                            .rounded(cx.theme().radius)
                                                            .small(),
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
                            .child(self.render_comments(&issue_id, cx))
                            .child(self.render_form(&issue_id, cx)),
                    ),
            )
            .child(self.render_sidebar(cx))
            .into_any_element()
    }
}

fn sidebar_title(text: &str, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
        .into_any_element()
}
