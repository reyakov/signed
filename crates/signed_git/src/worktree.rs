use std::path::PathBuf;

use crate::history::FileCommit;

pub struct WorktreeSnapshot {
    pub entries: Vec<PathBuf>,
    pub readme_path: Option<PathBuf>,
    pub readme: Option<Vec<u8>>,
    pub current_branch: Option<String>,
    pub head_commit: Option<FileCommit>,
    pub branches: Vec<String>,
    pub tags: Vec<String>,
}

impl WorktreeSnapshot {
    /// Creates a snapshot of the worktree contents and ref state.
    pub(crate) fn new(
        entries: Vec<PathBuf>,
        readme_path: Option<PathBuf>,
        readme: Option<Vec<u8>>,
        current_branch: Option<String>,
        head_commit: Option<FileCommit>,
        branches: Vec<String>,
        tags: Vec<String>,
    ) -> Self {
        Self {
            entries,
            readme_path,
            readme,
            current_branch,
            head_commit,
            branches,
            tags,
        }
    }
}
