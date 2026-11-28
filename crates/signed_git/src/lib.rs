mod cache;
mod diff;
mod history;
mod nip34;
mod patch;
mod repo;
mod scan;
mod sync;
mod worktree;

pub use cache::GitCache;
pub use diff::{CommitDiff, DiffHunk, DiffLine, DiffLineKind, DiffStatus, FileDiff};
pub use history::{CommitList, FileCommit, MAX_LISTED_COMMITS};
pub use nip34::{GraspSignals, Nip34Binding, Nip34Kind};
pub use patch::PatchParser;
pub use repo::{Repo, RepoRefState};
pub use scan::LocalRepo;
pub use sync::{RefSync, RepoSyncStatus};
pub use worktree::WorktreeSnapshot;

pub(crate) trait GixResultExt<T> {
    /// Converts a gix exception result into an `anyhow` result.
    fn into_anyhow(self) -> anyhow::Result<T>;
}

impl<T, E> GixResultExt<T> for Result<T, gix::Exn<E>>
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn into_anyhow(self) -> anyhow::Result<T> {
        self.map_err(|exn| anyhow::Error::from(exn.into_error()))
    }
}
