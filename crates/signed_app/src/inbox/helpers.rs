use std::sync::Arc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, Div, Entity, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, Stateful, Styled as _, div,
};
use metadata::ProfileDirectory;
use nostr::prelude::{Event, Kind, PublicKey};
use ui::avatar::Avatar;
use ui::badge::Badge;
use ui::common::{Sizable as _, h_flex, v_flex};
use ui::status_dot::{StatusDot, StatusDotVariant};
use ui::theme::ActiveTheme as _;
use utils::relative_time;

use super::{EntryRow, InboxSection};

impl InboxSection {
    /// The section's header row: its repository name and unread count.
    pub(super) fn header(&self, _cx: &App) -> Stateful<Div> {
        h_flex()
            .id((
                gpui::ElementId::from(self.badge_id.clone()),
                SharedString::from("header"),
            ))
            .h_12()
            .w_full()
            .gap_2()
            .child(
                div()
                    .min_w_0()
                    .text_sm()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(self.name.clone()),
            )
            .when(self.unread > 0, |this| {
                this.child(Badge::new(self.badge_id.clone(), self.unread.to_string()))
            })
    }

    /// The placeholder shown when the section has no threads.
    pub(super) fn empty_row(cx: &App) -> Stateful<Div> {
        h_flex()
            .id("inbox-empty")
            .h_12()
            .w_full()
            .px_3()
            .text_xs()
            .text_color(cx.theme().colors.muted_foreground)
            .bg(cx.theme().colors.secondary)
            .rounded(cx.theme().radius.control)
            .child(SharedString::from("No activity yet."))
    }
}

/// One sub-activity line under a thread's title.
#[derive(IntoElement)]
pub(super) struct ActivityRow {
    events: Arc<[Event]>,
    ix: usize,
    me: Option<PublicKey>,
    directory: Entity<ProfileDirectory>,
}

impl ActivityRow {
    pub(super) fn new(
        events: &Arc<[Event]>,
        ix: usize,
        me: Option<PublicKey>,
        directory: &Entity<ProfileDirectory>,
    ) -> Self {
        Self {
            events: Arc::clone(events),
            ix,
            me,
            directory: directory.clone(),
        }
    }
}

impl RenderOnce for ActivityRow {
    fn render(self, _window: &mut gpui::Window, cx: &mut App) -> impl IntoElement {
        let Some(event) = self.events.get(self.ix) else {
            return div();
        };
        let profile = self.directory.read(cx).profile(&event.pubkey, cx);
        let name = if Some(event.pubkey) == self.me {
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
    }
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

impl super::InboxView {
    /// One thread row, keyed by its thread root.
    pub(super) fn thread(&self, row: &EntryRow, me: Option<PublicKey>, cx: &App) -> Stateful<Div> {
        v_flex()
            // Keyed by the thread, not its position: rows reorder by latest activity.
            .id((
                gpui::ElementId::from("inbox-row"),
                SharedString::from(row.root.to_hex()),
            ))
            .w_full()
            .px_3()
            .py_2()
            .gap_2()
            .bg(cx.theme().colors.secondary)
            .when(row.first, |this| this.rounded_t(cx.theme().radius.control))
            .when(row.last, |this| this.rounded_b(cx.theme().radius.control))
            .when(!row.last, |this| {
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
                                ui::icons::Icon::named(ui::icons::IconRole::Inbox)
                                    .small()
                                    .color(cx.theme().colors.muted_foreground),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(row.title.clone()),
                    )
                    .child(div().flex_1())
                    .when(row.unread, |this| {
                        this.child(StatusDot::new().variant(StatusDotVariant::Accent))
                    }),
            )
            .child(
                v_flex().gap_2().w_full().children(
                    (0..row.timeline.len())
                        .map(|ix| ActivityRow::new(&row.timeline, ix, me, &self.directory)),
                ),
            )
    }
}
