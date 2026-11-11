use assets::CustomIconName;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Entity, IntoElement, ParentElement as _, SharedString, Styled as _, div, px,
};
use gpui_native_kit::avatar::Avatar;
use gpui_native_kit::badge::{Badge, BadgeVariant};
use gpui_native_kit::button::Button;
use gpui_native_kit::common::{Sizable as _, h_flex, v_flex};
use gpui_native_kit::icons::Icon;
use gpui_native_kit::input::{Input, InputState};
use gpui_native_kit::theme::ActiveTheme as _;
use gpui_native_kit::tooltip::Tooltip;
use metadata::ProfileDirectory;
use nostr::prelude::{Event, EventId, PublicKey};
use signed_state::RepoStore;
use utils::relative_time;

pub fn issue_roots(store: &RepoStore) -> &[Event] {
    &store.issues
}

fn sidebar_title(text: &str, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(cx.theme().colors.muted_foreground)
        .child(text.to_string())
        .into_any_element()
}

pub fn sidebar_section(
    store: &Entity<RepoStore>,
    id: EventId,
    roots: fn(&RepoStore) -> &[Event],
    top_gap: bool,
    directory: &Entity<ProfileDirectory>,
    cx: &App,
) -> AnyElement {
    let store = store.read(cx);
    let Some(root) = roots(store).iter().find(|event| event.id == id) else {
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
                .child(sidebar_title("Participants", cx))
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
                        .child(div().text_sm().text_ellipsis().child(name))
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

pub fn comments_section(
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
        .child(
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title),
        )
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
                        .text_sm()
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
                .child(div().text_sm().child(content))
        }))
        .into_any_element()
}

/// `roots` selects the root's list within the store, issues or pull requests.
pub fn comment_form(
    store: &Entity<RepoStore>,
    root: EventId,
    roots: fn(&RepoStore) -> &[Event],
    comment_input: &Entity<InputState>,
    button_id: &'static str,
    cx: &App,
) -> AnyElement {
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
                            Icon::named(CustomIconName::Markdown)
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
