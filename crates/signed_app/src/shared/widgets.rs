use nostr::prelude::Event;
use signed_core::RepoStatus;
use ui::badge::{Badge, BadgeVariant};
use ui::common::Sizable;
use ui::icons::{Icon, IconRole};
use ui::tooltip::Tooltip;

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

    /// Indices of `roots` whose status `keep` accepts, counting every root's status.
    pub(crate) fn filter<'a>(
        roots: impl IntoIterator<Item = &'a Event>,
        status_of: impl Fn(&Event) -> RepoStatus,
        keep: impl Fn(RepoStatus) -> bool,
    ) -> (Vec<usize>, Self) {
        let mut counts = Self::default();

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
}

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
        .small()
}
