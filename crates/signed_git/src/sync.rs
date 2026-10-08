use std::collections::BTreeMap;

/// How a single branch compares between local and Nostr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefSync {
    InSync,
    LocalAhead { ahead: usize },
    RemoteAhead { behind: usize },
    Diverged { ahead: usize, behind: usize },
    LocalOnly,
    RemoteOnly,
}

/// Branch comparison between a local repository and its Nostr state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSyncStatus {
    /// Branch short name and its classification.
    pub refs: Vec<(String, RefSync)>,
    pub ahead_total: usize,
    pub behind_total: usize,
}

impl RepoSyncStatus {
    /// True when every branch matches the Nostr state.
    pub fn in_sync(&self) -> bool {
        self.refs.iter().all(|(_, sync)| *sync == RefSync::InSync)
    }

    /// Maps branch short name to lowercase commit oid.
    pub(crate) fn branches(refs: &[(String, String)]) -> BTreeMap<&str, String> {
        refs.iter()
            .filter_map(|(name, commit)| {
                name.strip_prefix("refs/heads/")
                    .map(|branch| (branch, commit.to_ascii_lowercase()))
            })
            .collect()
    }
}
