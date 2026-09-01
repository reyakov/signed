use assets::CustomIconName;
use gpui::prelude::*;
use gpui::{AnyElement, App};
use gpui_component::tooltip::Tooltip;
use gpui_component::{ActiveTheme, Icon, Sizable, v_flex};
use signed_core::RepoStatus;

/// The status badge shown next to an issue or pull request: icon + colored square,
/// with a tooltip describing the status.
pub fn status_badge(status: RepoStatus, cx: &App) -> AnyElement {
    let (icon, label, tooltip, bg, fg) = match status {
        RepoStatus::Open => (
            CustomIconName::GitIssueDone,
            "open",
            "Issue is open",
            cx.theme().primary,
            cx.theme().primary_foreground,
        ),
        RepoStatus::Closed => (
            CustomIconName::GitIssueClosed,
            "closed",
            "Issue is closed",
            cx.theme().danger,
            cx.theme().danger_foreground,
        ),
        RepoStatus::Draft => (
            CustomIconName::GitIssueOngoing,
            "draft",
            "Issue is draft",
            cx.theme().accent,
            cx.theme().accent_foreground,
        ),
        RepoStatus::Applied => (
            CustomIconName::GitIssueOpen,
            "applied",
            "Issue is completed",
            cx.theme().secondary,
            cx.theme().secondary_foreground,
        ),
    };

    v_flex()
        .id(label)
        .flex_shrink_0()
        .size_7()
        .items_center()
        .justify_center()
        .rounded(cx.theme().radius)
        .bg(bg)
        .child(Icon::new(icon).small().text_color(fg))
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .into_any_element()
}
