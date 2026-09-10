use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// In-memory object cache for history walks, see [`open_with_cache`].
///
/// Without one, a walk re-decodes the same commit objects from the object database.
/// Sized generously: a walk can cover a large portion of the repository's history.
const OBJECT_CACHE_BYTES: usize = 64 * 1024 * 1024;

/// Metadata of a commit, as shown in the repository browser's file header.
#[derive(Debug, Clone)]
pub struct FileCommit {
    /// Shortened commit id, 7+ hex chars, disambiguated if needed.
    pub id: String,
    /// First line of the commit message.
    pub summary: String,
    /// Rest of the commit message after the title.
    ///
    /// `None` for single-line commit messages.
    pub description: Option<String>,
    /// Author name.
    pub author: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
}

/// Open the repository at `workdir` with an in-memory object cache.
///
/// Only history walks use it, they re-decode the same commit objects repeatedly.
/// Single-object reads open the repository plain.
pub(crate) fn open_with_cache(workdir: &Path) -> Result<gix::Repository> {
    let mut repo = gix::open(workdir)?;
    repo.object_cache_size_if_unset(OBJECT_CACHE_BYTES);
    Ok(repo)
}

/// A [`FileCommit`] from a commit, with author, message title, body and shortened id.
///
/// The diff panel fetches the full commit on demand.
fn file_commit(commit: &gix::Commit<'_>) -> Result<FileCommit> {
    file_commit_with_description(commit, true)
}

/// A [`FileCommit`] without the message body, for history lists that never display it.
///
/// Skipping the body saves an allocation per listed commit.
fn file_commit_summary(commit: &gix::Commit<'_>) -> Result<FileCommit> {
    file_commit_with_description(commit, false)
}

/// [`file_commit`] and [`file_commit_summary`], `include_description` picks the body.
fn file_commit_with_description(
    commit: &gix::Commit<'_>,
    include_description: bool,
) -> Result<FileCommit> {
    let author = commit.author()?;
    let message = commit.message()?;

    Ok(FileCommit {
        id: commit.id().shorten_or_id().to_string(),
        summary: String::from_utf8_lossy(message.title).trim().to_string(),
        description: if include_description {
            message
                .body
                .map(|body| String::from_utf8_lossy(body).trim().to_string())
                .filter(|body| !body.is_empty())
        } else {
            None
        },
        author: String::from_utf8_lossy(author.name).trim().to_string(),
        time: author.time()?.seconds,
    })
}

/// Find the most recent commit that changed `rel`, a path relative to the worktree.
///
/// `Ok(None)` when no commit touched the file, e.g. an untracked file.
pub fn last_commit(repo: &gix::Repository, rel: &Path) -> Result<Option<FileCommit>> {
    let rel = rel.to_path_buf();
    Ok(last_commits(repo, std::slice::from_ref(&rel))?
        .into_iter()
        .next()
        .map(|(_, commit)| commit))
}

/// Newest commit touching each of `rels`, like `git log -1 -- <rel>` per path.
/// `rels` are paths relative to the worktree.
///
/// Paths without any commit, like untracked files, are absent from the result.
pub fn worktree_last_commits(
    workdir: &Path,
    rels: &[PathBuf],
) -> Result<Vec<(PathBuf, FileCommit)>> {
    last_commits(&open_with_cache(workdir)?, rels)
}

/// The walk behind [`last_commit`] and [`worktree_last_commits`].
///
/// Stops as soon as every pending path has its commit.
fn last_commits(repo: &gix::Repository, rels: &[PathBuf]) -> Result<Vec<(PathBuf, FileCommit)>> {
    use gix::traverse::commit::simple::CommitTimeOrder;

    let Some(head) = repo.head_id().ok() else {
        return Ok(Vec::new());
    };

    // De-duplicate while preserving order.
    let mut pending: Vec<PathBuf> = Vec::with_capacity(rels.len());
    let mut seen: HashSet<&Path> = HashSet::with_capacity(rels.len());

    for rel in rels {
        if seen.insert(rel.as_path()) {
            pending.push(rel.clone());
        }
    }

    let walk = repo
        .rev_walk([head])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            CommitTimeOrder::NewestFirst,
        ));

    let mut found = Vec::new();
    for info in walk.all()? {
        if pending.is_empty() {
            break;
        }
        let info = info?;
        let commit = info.object()?;
        let tree = commit.tree()?;
        let parent_tree = match info.parent_ids().next() {
            Some(parent) => Some(parent.object()?.into_commit().tree()?),
            None => None,
        };

        // Compare each unresolved path against this commit and its first parent.
        // Resolved paths leave the pending set.
        let mut ix = 0;
        while ix < pending.len() {
            let rel = &pending[ix];
            let blob = tree.lookup_entry_by_path(rel)?;
            let parent_blob = match &parent_tree {
                Some(tree) => tree.lookup_entry_by_path(rel)?,
                None => None,
            };

            if blob.map(|entry| entry.id().detach()) != parent_blob.map(|entry| entry.id().detach())
            {
                found.push((rel.clone(), file_commit(&commit)?));
                pending.swap_remove(ix);
            } else {
                ix += 1;
            }
        }
    }

    Ok(found)
}

/// Cap on [`CommitList::commits`]. The virtual list renders a window at a time,
/// the tab badge shows the real count.
///
/// A huge history is never fully materialized in memory.
pub const MAX_LISTED_COMMITS: usize = 20_000;

/// Commits reachable from `HEAD`, newest first, possibly capped.
pub struct CommitList {
    /// Number of commits reachable from HEAD.
    pub total: usize,
    /// Newest commits, capped at [`MAX_LISTED_COMMITS`].
    pub commits: Vec<FileCommit>,
}

/// All commits reachable from `HEAD`, newest first, with author and summary.
///
/// Returns an empty list for a repository without any commits yet.
pub fn all_commits(repo: &gix::Repository) -> Result<CommitList> {
    use gix::traverse::commit::simple::CommitTimeOrder;

    let Some(head) = repo.head_id().ok() else {
        return Ok(CommitList {
            total: 0,
            commits: Vec::new(),
        });
    };

    let walk = repo
        .rev_walk([head])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            CommitTimeOrder::NewestFirst,
        ));

    let mut commits = Vec::new();
    let mut total = 0;

    for info in walk.all()? {
        let info = info?;
        total += 1;
        if commits.len() < MAX_LISTED_COMMITS {
            commits.push(file_commit_summary(&info.object()?)?);
        }
    }

    Ok(CommitList { total, commits })
}

/// Like [`all_commits`], but opens the repository at `workdir` first.
///
/// For non-bare clones the clone root is the worktree.
pub fn worktree_all_commits(workdir: &Path) -> Result<CommitList> {
    all_commits(&open_with_cache(workdir)?)
}

/// Commits in the range `base`..`tip`, newest first, like `git log base..tip`.
pub fn worktree_commit_range_commits(
    workdir: &Path,
    base: &str,
    tip: &str,
) -> Result<Vec<FileCommit>> {
    use gix::traverse::commit::simple::CommitTimeOrder;

    let repo = open_with_cache(workdir)?;
    let base_id = repo.rev_parse_single(base.as_bytes())?;
    let tip_id = repo.rev_parse_single(tip.as_bytes())?;
    let walk = repo
        .rev_walk([tip_id])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            CommitTimeOrder::NewestFirst,
        ))
        .with_hidden([base_id]);

    let mut commits = Vec::new();

    for info in walk.all()? {
        let info = info?;
        commits.push(file_commit_summary(&info.object()?)?);
    }

    Ok(commits)
}
/// The commit HEAD points to, like `git log -1`.
///
/// `Ok(None)` for a repository without commits yet, an unborn HEAD.
pub fn head_commit(repo: &gix::Repository) -> Result<Option<FileCommit>> {
    let Some(head) = repo.head_id().ok() else {
        return Ok(None);
    };
    let commit = head.object()?.into_commit();
    Ok(Some(file_commit(&commit)?))
}

/// Full metadata of the commit `id`, short or full, in the repository at `workdir`.
/// Like [`head_commit`] for an arbitrary commit.
///
/// `Ok(None)` when the id cannot be resolved.
pub fn worktree_commit(workdir: &Path, id: &str) -> Result<Option<FileCommit>> {
    let repo = gix::open(workdir)?;
    match repo.rev_parse_single(id.as_bytes()) {
        Ok(commit_id) => {
            let commit = commit_id.object()?.into_commit();
            Ok(Some(file_commit(&commit)?))
        }
        Err(_) => Ok(None),
    }
}
