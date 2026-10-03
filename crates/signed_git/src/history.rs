use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::repo::Repo;

#[derive(Debug, Clone)]
pub struct FileCommit {
    pub id: String,
    pub summary: String,
    pub description: Option<String>,
    pub author: String,
    pub time: i64,
}

impl FileCommit {
    fn from_commit(commit: &gix::Commit<'_>) -> Result<FileCommit> {
        Self::from_commit_with_description(commit, true)
    }

    // History lists never display the body,
    // skipping it saves an allocation per listed commit.
    fn from_commit_summary(commit: &gix::Commit<'_>) -> Result<FileCommit> {
        Self::from_commit_with_description(commit, false)
    }

    fn from_commit_with_description(
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
}

impl Repo {
    // Paths without any commit, like untracked files, are absent from the result.
    pub fn last_commits(&self, rels: &[PathBuf]) -> Result<Vec<(PathBuf, FileCommit)>> {
        use gix::traverse::commit::simple::CommitTimeOrder;

        let Some(head) = self.inner.head_id().ok() else {
            return Ok(Vec::new());
        };

        let mut pending: Vec<PathBuf> = Vec::with_capacity(rels.len());
        let mut seen: HashSet<&Path> = HashSet::with_capacity(rels.len());

        for rel in rels {
            if seen.insert(rel.as_path()) {
                pending.push(rel.clone());
            }
        }

        let walk = self
            .inner
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
            let mut ix = 0;
            while ix < pending.len() {
                let rel = &pending[ix];
                let blob = tree.lookup_entry_by_path(rel)?;
                let parent_blob = match &parent_tree {
                    Some(tree) => tree.lookup_entry_by_path(rel)?,
                    None => None,
                };

                if blob.map(|entry| entry.id().detach())
                    != parent_blob.map(|entry| entry.id().detach())
                {
                    found.push((rel.clone(), FileCommit::from_commit(&commit)?));
                    pending.swap_remove(ix);
                } else {
                    ix += 1;
                }
            }
        }

        Ok(found)
    }

    pub fn all_commits(&self) -> Result<CommitList> {
        use gix::traverse::commit::simple::CommitTimeOrder;

        let Some(head) = self.inner.head_id().ok() else {
            return Ok(CommitList {
                total: 0,
                commits: Vec::new(),
            });
        };

        let walk = self
            .inner
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
                commits.push(FileCommit::from_commit_summary(&info.object()?)?);
            }
        }

        Ok(CommitList { total, commits })
    }

    pub fn commit_range(&self, base: &str, tip: &str) -> Result<Vec<FileCommit>> {
        use gix::traverse::commit::simple::CommitTimeOrder;

        let base_id = self.inner.rev_parse_single(base.as_bytes())?;
        let tip_id = self.inner.rev_parse_single(tip.as_bytes())?;
        let walk = self
            .inner
            .rev_walk([tip_id])
            .sorting(gix::revision::walk::Sorting::ByCommitTime(
                CommitTimeOrder::NewestFirst,
            ))
            .with_hidden([base_id]);

        let mut commits = Vec::new();

        for info in walk.all()? {
            let info = info?;
            commits.push(FileCommit::from_commit_summary(&info.object()?)?);
        }

        Ok(commits)
    }

    // `Ok(None)` for an unborn HEAD.
    pub fn head_commit(&self) -> Result<Option<FileCommit>> {
        let Some(head) = self.inner.head_id().ok() else {
            return Ok(None);
        };
        let commit = head.object()?.into_commit();
        Ok(Some(FileCommit::from_commit(&commit)?))
    }

    // `Ok(None)` when the id cannot be resolved.
    pub fn commit(&self, id: &str) -> Result<Option<FileCommit>> {
        match self.inner.rev_parse_single(id.as_bytes()) {
            Ok(commit_id) => {
                let commit = commit_id.object()?.into_commit();
                Ok(Some(FileCommit::from_commit(&commit)?))
            }
            Err(_) => Ok(None),
        }
    }
}

// The virtual list renders a window at a time, the tab badge shows the real
// count: a huge history is never fully materialized in memory.
pub const MAX_LISTED_COMMITS: usize = 20_000;

pub struct CommitList {
    pub total: usize,
    pub commits: Vec<FileCommit>,
}
