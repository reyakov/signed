use assets::CustomIconName;
use gpui::prelude::*;
use gpui::{AnyElement, App, Entity, SharedString, div, px};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Textarea, TextareaState};
use gpui_component::tag::Tag;
use gpui_component::{ActiveTheme, Icon, Sizable, StyledExt, h_flex, v_flex};
use nostr::prelude::{Event, EventId, PublicKey};
use signed_state::{ProfileStore, RepoStore};
use signed_ui::UserAvatar;
use utils::relative_time;

pub(crate) fn issue_roots(store: &RepoStore) -> &[Event] {
    &store.issues
}

pub(crate) fn pr_roots(store: &RepoStore) -> &[Event] {
    &store.pull_requests
}

fn sidebar_title(text: &str, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(text.to_string())
        .into_any_element()
}

pub(crate) fn sidebar_section(
    store: &Entity<RepoStore>,
    id: EventId,
    roots: fn(&RepoStore) -> &[Event],
    top_gap: bool,
    cx: &App,
) -> AnyElement {
    let store = store.read(cx);
    let Some(root) = roots(store).iter().find(|event| event.id == id) else {
        // The caller bails out when the root is missing.
        return div().into_any_element();
    };
    let profile_store = ProfileStore::global(cx);

    // Participants, the root author plus everyone who commented.
    let mut participants: Vec<PublicKey> = vec![root.pubkey];
    participants.extend(store.comments_of(&root.id).map(|comment| comment.pubkey));
    participants.sort_by_key(PublicKey::to_hex);
    participants.dedup();

    // Labels are NIP-34 `t` hashtag tags on the event.
    let labels: Vec<String> = root.tags.hashtags().map(|tag| tag.to_string()).collect();

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
                .when(top_gap, |this| this.mt_4())
                .gap_2()
                .child(sidebar_title("Participants", cx))
                .children(participants.iter().map(|pubkey| {
                    let profile = profile_store.read(cx).get(pubkey);
                    let name = profile.name();
                    let picture = profile.picture();

                    h_flex()
                        .gap_1()
                        .items_center()
                        .child(UserAvatar::new(name.clone()).picture(picture))
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

pub(crate) fn comments_section(store: &Entity<RepoStore>, root: EventId, cx: &App) -> AnyElement {
    let store = store.read(cx);
    let comments: Vec<&Event> = store.comments_of(&root).collect();
    let title = SharedString::from(format!("Discussions {}", comments.len()));

    v_flex()
        .gap_4()
        .child(div().text_xs().font_semibold().child(title))
        .children(comments.iter().map(|comment| {
            let profile = ProfileStore::global(cx).read(cx).get(&comment.pubkey);
            let author = profile.name();
            let picture = profile.picture();
            let age = relative_time(comment.created_at);
            let content = SharedString::from(comment.content.as_str());

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
                                .child(UserAvatar::new(author.clone()).picture(picture))
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

/// `roots` selects the root's list within the store, issues or pull requests.
pub(crate) fn comment_form(
    store: &Entity<RepoStore>,
    root: EventId,
    roots: fn(&RepoStore) -> &[Event],
    comment_input: &Entity<TextareaState>,
    button_id: &'static str,
    cx: &App,
) -> AnyElement {
    let comment_input = comment_input.clone();
    let store = store.clone();

    v_flex()
        .gap_2()
        .child(
            Textarea::new(&comment_input)
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
                    Button::new(button_id)
                        .primary()
                        .label("Comment")
                        .tooltip("Post comment")
                        .on_click(move |_event, window, cx| {
                            let content = comment_input.read(cx).value().trim().to_string();
                            if content.is_empty() {
                                return;
                            }
                            let Some(root) = roots(store.read(cx))
                                .iter()
                                .find(|event| event.id == root)
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
