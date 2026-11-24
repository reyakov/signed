use signed_core::RepoStatus;
use ui::badge::{Badge, BadgeVariant};
use ui::icons::{Icon, IconRole};
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
