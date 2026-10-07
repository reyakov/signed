use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ignore::WalkBuilder;

use crate::nip34::Nip34Binding;
use crate::repo::Repo;

const SCAN_MAX_DEPTH: usize = 12;

#[derive(Debug, Clone)]
pub struct LocalRepo {
    pub path: PathBuf,
    pub nip34: Option<Nip34Binding>,
}

impl LocalRepo {
    /// Creates a local repository entry from its path and optional NIP-34 binding.
    pub fn new(path: PathBuf, nip34: Option<Nip34Binding>) -> Self {
        Self { path, nip34 }
    }
}

/// Finds git repositories under `root` up to `SCAN_MAX_DEPTH` deep, without descending into them.
pub fn find_git_repos(root: &Path) -> Vec<LocalRepo> {
    let Ok(root) = root.canonicalize() else {
        return Vec::new();
    };

    let found = Arc::new(Mutex::new(Vec::<PathBuf>::new()));

    let walker = WalkBuilder::new(&root)
        .max_depth(Some(SCAN_MAX_DEPTH))
        .require_git(false)
        .filter_entry({
            let found = found.clone();
            move |entry| {
                let Some(kind) = entry.file_type() else {
                    return true;
                };
                if !kind.is_dir() {
                    return true;
                }

                let dir = entry.path();
                let Ok(repo) = gix::discover(dir) else {
                    return true;
                };

                let Some(workdir) = repo.workdir() else {
                    return false;
                };

                if workdir != dir {
                    return true;
                }

                match found.lock() {
                    Ok(mut repos) => repos.push(dir.to_path_buf()),
                    Err(poisoned) => poisoned.into_inner().push(dir.to_path_buf()),
                }
                false
            }
        })
        .build();

    for _ in walker {}

    let mut repos = match found.lock() {
        Ok(mut repos) => std::mem::take(&mut *repos),
        Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
    };
    repos.sort();

    repos
        .into_iter()
        .map(|path| {
            let nip34 = Repo::open(&path).ok().and_then(|repo| repo.nip34_binding());
            LocalRepo::new(path, nip34)
        })
        .collect()
}
