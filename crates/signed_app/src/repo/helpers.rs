use std::path::{Path, PathBuf};

use anyhow::Error;
use gpui::{Div, ParentElement as _, SharedString, Styled as _, div};
use signed_git::{FileCommit, Repo, WorktreeSnapshot};
use ui::clipboard::CopyRow;
use ui::common::{StyledExt as _, v_flex};
use ui::tree::TreeItemSeed;

/// The worktree-derived data the repository panel is built from.
pub(super) struct RepoData {
    pub(super) tree: Vec<TreeItemSeed>,
    pub(super) entries: Vec<PathBuf>,
    pub(super) readme_path: Option<PathBuf>,
    pub(super) readme: Option<Vec<u8>>,
    pub(super) worktree: Option<PathBuf>,
    pub(super) branches: Vec<String>,
    pub(super) tags: Vec<String>,
    pub(super) current_branch: Option<String>,
    pub(super) head_commit: Option<FileCommit>,
}

impl RepoData {
    /// Snapshots the repository and derives the explorer's tree rows.
    pub(super) fn load(repo: &Repo) -> Result<Self, Error> {
        let snapshot = repo.snapshot()?;
        let worktree = repo.workdir().map(Path::to_path_buf);
        let tree = TreeItemSeed::from_paths(&snapshot.entries);

        Ok(Self {
            tree,
            entries: snapshot.entries,
            readme_path: snapshot.readme_path,
            readme: snapshot.readme,
            worktree,
            branches: snapshot.branches,
            tags: snapshot.tags,
            current_branch: snapshot.current_branch,
            head_commit: snapshot.head_commit,
        })
    }

    /// Snapshots the worktree and derives the explorer's tree rows and paths.
    pub(super) fn snapshot(
        worktree: PathBuf,
    ) -> Result<(WorktreeSnapshot, Vec<TreeItemSeed>, Vec<String>), Error> {
        let snapshot = Repo::open(&worktree)?.snapshot()?;
        let tree = TreeItemSeed::from_paths(&snapshot.entries);
        let paths = CommitTree::sorted_paths(&snapshot.entries);
        Ok((snapshot, tree, paths))
    }
}

/// A commit's tree and README, as the file explorer shows them.
pub(super) struct CommitTree {
    pub(super) tree: Vec<TreeItemSeed>,
    pub(super) paths: Vec<String>,
    pub(super) readme_path: Option<PathBuf>,
    pub(super) readme: Option<Vec<u8>>,
}

impl CommitTree {
    /// Sorted relative paths of a snapshot's entries.
    pub(super) fn sorted_paths(entries: &[PathBuf]) -> Vec<String> {
        let mut paths: Vec<String> = entries
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        paths.sort();
        paths
    }

    /// Lists the tree and README of `commit` in the repository at `worktree`.
    pub(super) fn snapshot(worktree: &Path, commit: &str) -> Result<Self, Error> {
        let repo = Repo::open_cached(worktree)?;
        let entries = repo.entries_at(commit)?;
        let readme_path = Repo::pick_readme(&entries);
        let readme = match &readme_path {
            Some(path) => repo.read_at(commit, path)?,
            None => None,
        };

        Ok(Self {
            tree: TreeItemSeed::from_paths(&entries),
            paths: Self::sorted_paths(&entries),
            readme_path,
            readme,
        })
    }
}

pub(super) fn clone_group(label: &str, id: SharedString, command: &SharedString) -> Div {
    v_flex()
        .gap_1()
        .min_w_0()
        .child(
            div()
                .text_xs()
                .font_semibold()
                .child(SharedString::from(label)),
        )
        .child(CopyRow::new(id, command.clone()))
}
