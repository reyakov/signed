use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Entity, IntoElement, ParentElement as _, SharedString, Styled as _, div, px,
};
use metadata::ProfileDirectory;
use nostr::prelude::{Event, EventId, PublicKey};
use signed_core::RepoStatus;
use signed_state::RepoStore;
use ui::avatar::Avatar;
use ui::badge::{Badge, BadgeVariant};
use ui::button::Button;
use ui::common::{Sizable as _, StyledExt as _, h_flex, v_flex};
use ui::icons::{Icon, IconRole};
use ui::input::{Input, InputState};
use ui::theme::ActiveTheme as _;
use ui::tooltip::Tooltip;
use utils::relative_time;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IssueFilter {
    All,
    Open,
    Closed,
}

impl IssueFilter {
    pub(crate) fn matches(self, status: RepoStatus) -> bool {
        match self {
            Self::All => true,
            Self::Open => status == RepoStatus::Open,
            Self::Closed => matches!(status, RepoStatus::Closed | RepoStatus::Applied),
        }
    }
}

/// Root events counted by their resolved status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StatusCounts {
    pub(crate) total: usize,
    pub(crate) open: usize,
    pub(crate) closed: usize,
    pub(crate) draft: usize,
    pub(crate) applied: usize,
}

impl StatusCounts {
    fn record(&mut self, status: RepoStatus) {
        self.total += 1;
        match status {
            RepoStatus::Open => self.open += 1,
            RepoStatus::Closed => self.closed += 1,
            RepoStatus::Draft => self.draft += 1,
            RepoStatus::Applied => self.applied += 1,
        }
    }
}

/// Indices of `roots` whose status `keep` accepts, counting every root's status.
pub(crate) fn filter_by_status<'a>(
    roots: impl IntoIterator<Item = &'a Event>,
    status_of: impl Fn(&Event) -> RepoStatus,
    keep: impl Fn(RepoStatus) -> bool,
) -> (Vec<usize>, StatusCounts) {
    let mut counts = StatusCounts::default();

    let visible = roots
        .into_iter()
        .enumerate()
        .filter_map(|(index, root)| {
            let status = status_of(root);
            counts.record(status);
            keep(status).then_some(index)
        })
        .collect();

    (visible, counts)
}

pub(crate) fn sidebar_section(
    store: &Entity<RepoStore>,
    id: EventId,
    roots: &[Event],
    top_gap: bool,
    directory: &Entity<ProfileDirectory>,
    cx: &App,
) -> AnyElement {
    let store = store.read(cx);
    let Some(root) = roots.iter().find(|event| event.id == id) else {
        return div().into_any_element();
    };

    let mut participants: Vec<PublicKey> = vec![root.pubkey];
    participants.extend(store.comments_of(&root.id).map(|comment| comment.pubkey));
    participants.sort_by_key(PublicKey::to_hex);
    participants.dedup();

    let labels: Vec<String> = root.tags.hashtags().map(|tag| tag.to_string()).collect();

    v_flex()
        .w(px(240.))
        .h_full()
        .flex_none()
        .px_4()
        .gap_4()
        .border_l_1()
        .border_color(cx.theme().colors.border)
        .child(
            v_flex()
                .when(top_gap, |this| this.mt_4())
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .font_medium()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child("Participants"),
                )
                .children(participants.iter().map(|pubkey| {
                    let profile = directory.read(cx).profile(pubkey, cx);
                    let name = profile.name();

                    h_flex()
                        .gap_1()
                        .child(
                            Avatar::new(*pubkey)
                                .name(name.clone())
                                .picture(profile.picture())
                                .small(),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .whitespace_nowrap()
                                .text_base()
                                .text_ellipsis()
                                .child(name),
                        )
                        .into_any_element()
                })),
        )
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .font_medium()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child("Labels"),
                )
                .map(|this| {
                    if labels.is_empty() {
                        this.child(
                            div()
                                .text_base()
                                .text_color(cx.theme().colors.muted_foreground)
                                .child("None yet."),
                        )
                    } else {
                        this.child(v_flex().gap_1().children(labels.iter().map(|label| {
                            Badge::new(SharedString::from(format!("label:{label}")), label.clone())
                                .variant(BadgeVariant::Outline)
                        })))
                    }
                }),
        )
        .into_any_element()
}

pub(crate) fn comments_section(
    store: &Entity<RepoStore>,
    root: EventId,
    directory: &Entity<ProfileDirectory>,
    cx: &App,
) -> AnyElement {
    let store = store.read(cx);
    let comments: Vec<&Event> = store.comments_of(&root).collect();
    let title = SharedString::from(format!("Discussions {}", comments.len()));

    v_flex()
        .gap_4()
        .child(div().text_xs().font_medium().child(title))
        .children(comments.iter().map(|comment| {
            let profile = directory.read(cx).profile(&comment.pubkey, cx);
            let author = profile.name();
            let age = relative_time(comment.created_at);
            let content = SharedString::from(comment.content.as_str());

            v_flex()
                .gap_1()
                .p_3()
                .border_1()
                .border_color(cx.theme().colors.border)
                .rounded(cx.theme().radius.control)
                .child(
                    h_flex()
                        .gap_2()
                        .text_base()
                        .child(
                            h_flex()
                                .gap_1()
                                .child(
                                    Avatar::new(comment.pubkey)
                                        .name(author.clone())
                                        .picture(profile.picture())
                                        .small(),
                                )
                                .child(author),
                        )
                        .child(
                            div()
                                .text_color(cx.theme().colors.muted_foreground)
                                .child("commented"),
                        )
                        .child(
                            div()
                                .text_color(cx.theme().colors.muted_foreground)
                                .child(SharedString::from(age)),
                        ),
                )
                .child(div().text_base().child(content))
        }))
        .into_any_element()
}

pub(crate) fn comment_form(
    store: &Entity<RepoStore>,
    root: EventId,
    roots: &[Event],
    comment_input: &Entity<InputState>,
    button_id: &'static str,
    cx: &App,
) -> AnyElement {
    let Some(root) = roots.iter().find(|event| event.id == root).cloned() else {
        return div().into_any_element();
    };
    let comment_input = comment_input.clone();
    let store = store.clone();

    v_flex()
        .gap_2()
        .child(
            Input::new("comment-input", comment_input.clone())
                .rows(4)
                .placeholder("Leave a comment..."),
        )
        .child(
            h_flex()
                .justify_between()
                .child(
                    h_flex()
                        .gap_1()
                        .text_xs()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(
                            Icon::named(IconRole::Markdown)
                                .size(px(12.))
                                .color(cx.theme().colors.muted_foreground),
                        )
                        .child("Markdown is supported"),
                )
                .child(
                    Button::new(button_id)
                        .label("Comment")
                        .tooltip(Tooltip::text("Post comment"))
                        .on_click(move |_, window, cx| {
                            let content = comment_input.read(cx).value().trim().to_string();
                            if content.is_empty() {
                                return;
                            }
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
