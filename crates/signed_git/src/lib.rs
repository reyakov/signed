mod cache;
mod diff;
mod history;
mod nip34;
mod patch;
mod remote;
mod repo;
mod scan;
mod worktree;

#[cfg(test)]
mod tests;

pub use cache::GitCache;
pub use diff::{CommitDiff, DiffHunk, DiffLine, DiffLineKind, DiffStatus, FileDiff};
pub use history::{CommitList, FileCommit, MAX_LISTED_COMMITS};
pub use nip34::{GraspSignals, Nip34Binding, Nip34Kind};
pub use patch::PatchParser;
pub use repo::{Repo, RepoRefState};
pub use scan::{LocalRepo, find_git_repos};
pub use worktree::WorktreeSnapshot;

pub(crate) trait GixResultExt<T> {
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

#[cfg(test)]
fn git_in(dir: &std::path::Path, args: &[&str]) -> anyhow::Result<String> {
    let output = Repo::run_git(dir, args, "git")?;

    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
