use gpui::{AnyElement, App, IntoElement, ParentElement as _, SharedString, Styled as _, div};
use signed_core::RepoStatus;
use ui::badge::{Badge, BadgeVariant};
use ui::empty_state::EmptyState;
use ui::icons::{Icon, IconRole};
use ui::theme::{ActiveTheme as _, TextRole, Typeset as _};
use ui::tooltip::Tooltip;

/// The status pill shown on issue and pull-request rows.
pub fn status_badge(status: RepoStatus) -> Badge {
    let (label, variant, icon) = match status {
        RepoStatus::Open => ("Open", BadgeVariant::Neutral, IconRole::GitIssueOngoing),
        RepoStatus::Closed => ("Closed", BadgeVariant::Danger, IconRole::GitIssueClosed),
        RepoStatus::Draft => ("Draft", BadgeVariant::Warning, IconRole::GitIssueOngoing),
        RepoStatus::Applied => ("Applied", BadgeVariant::Success, IconRole::GitIssueDone),
    };

    Badge::new(format!("status-{label}"), label)
        .variant(variant)
        .icon(Icon::named(icon))
        .tooltip(Tooltip::text(label))
}

/// A centered empty-state message filling the available space.
pub fn empty_state(message: impl Into<SharedString>) -> AnyElement {
    let message = message.into();
    ui::common::v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .child(EmptyState::new(message))
        .into_any_element()
}

/// A one-line muted label used by section headers.
pub fn section_label(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .typeset(TextRole::Caption2, cx)
        .text_color(cx.theme().colors.muted_foreground)
        .child(text.into())
        .into_any_element()
}
