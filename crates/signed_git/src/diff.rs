use std::path::Path;

use anyhow::Result;
use gix::diff::blob::unified_diff::{ConsumeHunk, DiffLineKind as GixLineKind, HunkHeader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    /// An unchanged context line, present on both sides.
    Context,
    Addition,
    Deletion,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// 1-based line number in the old version, if the line exists there.
    pub old: Option<u32>,
    /// 1-based line number in the new version, if the line exists there.
    pub new: Option<u32>,
    /// Line content without the trailing newline.
    pub text: String,
}

/// A hunk of a file diff, like `@@ -a,b +c,d @@`.
#[derive(Debug, Clone)]
pub struct DiffHunk {
    /// 1-based start line in the old version.
    pub old_start: u32,
    pub old_lines: u32,
    /// 1-based start line in the new version.
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
}

#[derive(Debug, Clone)]
pub struct FileDiff {
    /// Path of the file relative to the repo root.
    ///
    /// For renames and copies, this is the destination path.
    pub path: String,
    /// Previous path, for renames and copies.
    pub old_path: Option<String>,
    pub status: DiffStatus,
    /// Number of added lines, 0 for binary files.
    pub insertions: usize,
    /// Number of removed lines, 0 for binary files.
    pub deletions: usize,
    /// True if either version is binary, then `hunks` is empty.
    pub binary: bool,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone)]
pub struct CommitDiff {
    pub files: Vec<FileDiff>,
}

/// The changes of the commit `id`, short or full, in the repository at `workdir`.
///
/// Compared against its first parent, the empty tree for the root commit.
pub fn worktree_commit_diff(workdir: &Path, id: &str) -> Result<CommitDiff> {
    commit_diff(&gix::open(workdir)?, id)
}

fn commit_diff(repo: &gix::Repository, id: &str) -> Result<CommitDiff> {
    let commit_id = repo.rev_parse_single(id.as_bytes())?;
    let commit = commit_id.object()?.into_commit();
    let new_tree = commit.tree()?;
    let old_tree = match commit.parent_ids().next() {
        Some(parent) => Some(parent.object()?.into_commit().tree()?),
        None => None,
    };
    tree_diff(repo, old_tree.as_ref(), &new_tree)
}

/// The changes between two commits, `base`..`tip`, like `git diff base tip`.
///
/// Directories and submodules are skipped, files are sorted by path.
pub fn worktree_commit_range_diff(workdir: &Path, base: &str, tip: &str) -> Result<CommitDiff> {
    let repo = gix::open(workdir)?;
    let base_tree = repo
        .rev_parse_single(base.as_bytes())?
        .object()?
        .into_commit()
        .tree()?;
    let tip_tree = repo
        .rev_parse_single(tip.as_bytes())?
        .object()?
        .into_commit()
        .tree()?;
    tree_diff(&repo, Some(&base_tree), &tip_tree)
}

fn tree_diff(
    repo: &gix::Repository,
    old_tree: Option<&gix::Tree<'_>>,
    new_tree: &gix::Tree<'_>,
) -> Result<CommitDiff> {
    use gix::diff::blob::platform::prepare_diff::Operation;
    use gix::object::tree::diff::Change;
    use gix::objs::tree::EntryKind;

    let changes = repo.diff_tree_to_tree(old_tree, Some(new_tree), None)?;

    let mut cache = repo.diff_resource_cache_for_tree_diff()?;
    let mut files = Vec::new();

    for change in changes {
        let attached = Change::from_change_ref(change.to_ref(), repo, repo);

        // Skip directory trees and submodule gitlinks, only files are listed.
        let (path, old_path, status) = match attached {
            Change::Addition {
                location,
                entry_mode,
                ..
            } if !matches!(entry_mode.kind(), EntryKind::Tree | EntryKind::Commit) => {
                (location.to_owned(), None, DiffStatus::Added)
            }
            Change::Deletion {
                location,
                entry_mode,
                ..
            } if !matches!(entry_mode.kind(), EntryKind::Tree | EntryKind::Commit) => {
                (location.to_owned(), None, DiffStatus::Deleted)
            }
            Change::Modification {
                location,
                previous_entry_mode,
                entry_mode,
                ..
            } if !matches!(entry_mode.kind(), EntryKind::Tree | EntryKind::Commit)
                && !matches!(
                    previous_entry_mode.kind(),
                    EntryKind::Tree | EntryKind::Commit
                ) =>
            {
                (location.to_owned(), None, DiffStatus::Modified)
            }
            Change::Rewrite {
                location,
                source_location,
                source_entry_mode,
                entry_mode,
                copy,
                ..
            } if !matches!(entry_mode.kind(), EntryKind::Tree | EntryKind::Commit)
                && !matches!(
                    source_entry_mode.kind(),
                    EntryKind::Tree | EntryKind::Commit
                ) =>
            {
                let status = if copy {
                    DiffStatus::Copied
                } else {
                    DiffStatus::Renamed
                };
                (
                    location.to_owned(),
                    Some(source_location.to_owned()),
                    status,
                )
            }
            _ => continue,
        };

        // Always diff with the built-in algorithm.
        // External diff drivers would shell out, out of scope for a read-only viewer.
        let platform = attached.diff(&mut cache)?;
        platform
            .resource_cache
            .options
            .skip_internal_diff_if_external_is_configured = true;
        let outcome = platform.resource_cache.prepare_diff()?;

        let (binary, hunks, insertions, deletions) = match outcome.operation {
            Operation::InternalDiff { algorithm } => {
                let input = outcome.interned_input();
                let diff = gix::diff::blob::diff_with_slider_heuristics(algorithm, &input);

                let mut hunks = Vec::new();
                let mut insertions = 0usize;
                let mut deletions = 0usize;
                let collector = HunkCollector {
                    hunks: &mut hunks,
                    insertions: &mut insertions,
                    deletions: &mut deletions,
                };
                gix::diff::blob::UnifiedDiff::new(&diff, &input, collector, Default::default())
                    .consume()?;
                (false, hunks, insertions, deletions)
            }
            Operation::SourceOrDestinationIsBinary => (true, Vec::new(), 0, 0),
            Operation::ExternalCommand { .. } => unreachable!("external diff drivers are disabled"),
        };

        files.push(FileDiff {
            path: String::from_utf8_lossy(&path).into_owned(),
            old_path: old_path.map(|p| String::from_utf8_lossy(&p).into_owned()),
            status,
            insertions,
            deletions,
            binary,
            hunks,
        });
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(CommitDiff { files })
}

/// Collects the hunks of one blob diff while tracking per-line numbers.
struct HunkCollector<'a> {
    hunks: &'a mut Vec<DiffHunk>,
    insertions: &'a mut usize,
    deletions: &'a mut usize,
}

impl ConsumeHunk for HunkCollector<'_> {
    type Out = ();

    fn consume_hunk(
        &mut self,
        header: HunkHeader,
        lines: &[(GixLineKind, &[u8])],
    ) -> std::io::Result<()> {
        let mut old_ln = header.before_hunk_start;
        let mut new_ln = header.after_hunk_start;
        let mut out = Vec::with_capacity(lines.len());

        for (kind, content) in lines {
            let text = String::from_utf8_lossy(content).into_owned();
            let line = match kind {
                GixLineKind::Context => {
                    let line = DiffLine {
                        kind: DiffLineKind::Context,
                        old: Some(old_ln),
                        new: Some(new_ln),
                        text,
                    };
                    old_ln += 1;
                    new_ln += 1;
                    line
                }
                GixLineKind::Remove => {
                    *self.deletions += 1;
                    let line = DiffLine {
                        kind: DiffLineKind::Deletion,
                        old: Some(old_ln),
                        new: None,
                        text,
                    };
                    old_ln += 1;
                    line
                }
                GixLineKind::Add => {
                    *self.insertions += 1;
                    let line = DiffLine {
                        kind: DiffLineKind::Addition,
                        old: None,
                        new: Some(new_ln),
                        text,
                    };
                    new_ln += 1;
                    line
                }
            };
            out.push(line);
        }

        self.hunks.push(DiffHunk {
            old_start: header.before_hunk_start,
            old_lines: header.before_hunk_len,
            new_start: header.after_hunk_start,
            new_lines: header.after_hunk_len,
            lines: out,
        });

        Ok(())
    }

    fn finish(self) {}
}
