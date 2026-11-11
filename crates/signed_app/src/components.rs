use assets::CustomIconName;
use gpui_native_kit::badge::{Badge, BadgeVariant};
use gpui_native_kit::icons::Icon;
use gpui_native_kit::tooltip::Tooltip;
use signed_core::RepoStatus;

/// The status pill shown on issue and pull-request rows.
pub fn status_badge(status: RepoStatus) -> Badge {
    let (label, variant, icon) = match status {
        RepoStatus::Open => ("Open", BadgeVariant::Neutral, CustomIconName::GitIssueOpen),
        RepoStatus::Closed => (
            "Closed",
            BadgeVariant::Danger,
            CustomIconName::GitIssueClosed,
        ),
        RepoStatus::Draft => (
            "Draft",
            BadgeVariant::Warning,
            CustomIconName::GitIssueOngoing,
        ),
        RepoStatus::Applied => (
            "Applied",
            BadgeVariant::Success,
            CustomIconName::GitIssueDone,
        ),
    };

    Badge::new(format!("status-{label}"), label)
        .variant(variant)
        .icon(Icon::named(icon))
        .tooltip(Tooltip::text(label))
}
