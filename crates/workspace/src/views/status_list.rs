use nostr::prelude::Event;
use signed_core::RepoStatus;

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
