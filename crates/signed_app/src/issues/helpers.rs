use signed_core::RepoStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IssueFilter {
    All,
    Open,
    Closed,
}

impl IssueFilter {
    pub(crate) fn matches(self, status: RepoStatus) -> bool {
        match self {
            Self::All => true,
            Self::Open => status == RepoStatus::Open,
            Self::Closed => matches!(status, RepoStatus::Closed | RepoStatus::Applied),
        }
    }
}
