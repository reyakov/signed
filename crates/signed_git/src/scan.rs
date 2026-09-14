use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::nip34::{Nip34Binding, detect_nip34};

/// Caps nesting so pathological trees can't stall the scan.
const SCAN_MAX_DEPTH: usize = 12;

/// A git repository discovered under a scan root.
#[derive(Debug, Clone)]
pub struct LocalRepo {
    pub path: PathBuf,
    /// `None` for a plain repository.
    pub nip34: Option<Nip34Binding>,
}

/// Walk `root` recursively and collect the git repositories below it.
pub fn find_git_repos(root: &Path) -> Vec<LocalRepo> {
    if !root.is_dir() {
        return Vec::new();
    }

    let walker = WalkBuilder::new(root)
        .max_depth(Some(SCAN_MAX_DEPTH))
        // Honour `.gitignore` even when the scan root is not itself a repository.
        .require_git(false)
        .build();

    let mut repos: Vec<PathBuf> = walker
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_dir()))
        .map(ignore::DirEntry::into_path)
        .filter(|dir| dir.join(".git").exists())
        .filter_map(|dir| dir.canonicalize().ok())
        .collect();

    repos.sort();
    repos.dedup();

    // A repository nested inside another, like a submodule worktree, is not reported.
    let mut roots: Vec<PathBuf> = Vec::with_capacity(repos.len());
    for repo in repos {
        if !roots.iter().any(|kept| repo.starts_with(kept)) {
            roots.push(repo);
        }
    }

    roots
        .into_iter()
        .map(|path| {
            let nip34 = detect_nip34(&path);
            LocalRepo { path, nip34 }
        })
        .collect()
}
