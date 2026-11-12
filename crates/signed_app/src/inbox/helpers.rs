use assets::CustomIconName;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Div, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Stateful, Styled as _, div, px,
};
use gpui_native_kit::avatar::Avatar;
use gpui_native_kit::badge::Badge;
use gpui_native_kit::common::{Sizable as _, h_flex, v_flex};
use gpui_native_kit::icons::Icon;
use gpui_native_kit::theme::ActiveTheme as _;
use metadata::ProfileDirectory;
use nostr::prelude::{Event, Kind, PublicKey};
use utils::relative_time;

pub fn repo_header(name: SharedString, unread: usize) -> AnyElement {
    h_flex()
        .h_12()
        .w_full()
        .gap_2()
        .child(
            div()
                .min_w_0()
                .text_sm()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(name),
        )
        .when(unread > 0, |this| {
            this.child(Badge::new("inbox-unread", unread.to_string()))
        })
        .into_any_element()
}

pub fn empty_section_row(cx: &App) -> AnyElement {
    h_flex()
        .h_12()
        .w_full()
        .px_3()
        .text_xs()
        .text_color(cx.theme().colors.muted_foreground)
        .bg(cx.theme().colors.secondary)
        .rounded(cx.theme().radius.control)
        .child(SharedString::from("No activity yet."))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
pub fn thread(
    prefix: &'static str,
    ix: usize,
    title: SharedString,
    unread: bool,
    first: bool,
    last: bool,
    events: Vec<Event>,
    me: Option<PublicKey>,
    directory: &Entity<ProfileDirectory>,
    cx: &App,
) -> Stateful<Div> {
    let mut timeline = v_flex().gap_2().w_full();
    for event in events {
        timeline = timeline.child(sub_activity(&event, me, directory, cx));
    }

    v_flex()
        .id((prefix, ix))
        .w_full()
        .px_3()
        .py_2()
        .gap_2()
        .bg(cx.theme().colors.secondary)
        .when(first, |this| this.rounded_t(cx.theme().radius.control))
        .when(last, |this| this.rounded_b(cx.theme().radius.control))
        .when(!last, |this| {
            this.border_b_1().border_color(cx.theme().colors.background)
        })
        .hover(|this| this.bg(cx.theme().colors.muted_hover))
        .child(
            h_flex()
                .gap_2()
                .text_sm()
                .child(
                    h_flex()
                        .size_6()
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::named(CustomIconName::Inbox)
                                .size(px(14.))
                                .color(cx.theme().colors.muted_foreground),
                        ),
                )
                .child(
                    div()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(title),
                )
                .child(div().flex_1())
                .when(unread, |this| {
                    this.child(
                        div()
                            .flex_shrink_0()
                            .size_2()
                            .rounded_full()
                            .bg(cx.theme().colors.primary),
                    )
                }),
        )
        .child(timeline)
}

fn sub_activity(
    event: &Event,
    me: Option<PublicKey>,
    directory: &Entity<ProfileDirectory>,
    cx: &App,
) -> AnyElement {
    let profile = directory.read(cx).profile(&event.pubkey, cx);
    let name = if Some(event.pubkey) == me {
        SharedString::from("You")
    } else {
        profile.name()
    };

    h_flex()
        .w_full()
        .gap_2()
        .child(div().w_6().flex_shrink_0())
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .text_xs()
                .child(
                    Avatar::new(event.pubkey)
                        .name(name.clone())
                        .picture(profile.picture())
                        .small(),
                )
                .child(name)
                .child(SharedString::from(activity_phrase(event.kind)))
                .child(div().flex_1())
                .child(
                    div()
                        .text_color(cx.theme().colors.muted_foreground)
                        .child(SharedString::from(relative_time(event.created_at))),
                ),
        )
        .into_any_element()
}

fn activity_phrase(kind: Kind) -> &'static str {
    match kind {
        Kind::GitIssue => "opened an issue",
        Kind::GitPullRequest => "opened a PR",
        Kind::GitPullRequestUpdate => "updated a PR",
        Kind::GitPatch => "created a patch",
        Kind::Comment => "commented",
        Kind::GitStatusOpen => "opened a status",
        Kind::GitStatusApplied => "applied a status",
        Kind::GitStatusClosed => "closed a status",
        Kind::GitStatusDraft => "drafted a status",
        _ => "did something",
    }
}

pub fn empty_state(message: &'static str, cx: &App) -> AnyElement {
    v_flex()
        .w_full()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .gap_2()
        .py_8()
        .child(
            Icon::named(CustomIconName::Inbox)
                .size(px(24.))
                .color(cx.theme().colors.muted_foreground),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().colors.muted_foreground)
                .child(SharedString::from(message)),
        )
        .into_any_element()
}
