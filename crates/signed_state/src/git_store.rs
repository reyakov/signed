use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::Result;
use gix::Repository;
use signed_core::RepoAddr;
use signed_git::GitCache;

static GIT_CACHE: OnceLock<GitCache> = OnceLock::new();

fn git_cache() -> &'static GitCache {
    GIT_CACHE
        .get()
        .expect("git cache is initialized by signed_state::init")
}

/// The root directory of the repository mirrors.
pub(crate) fn repo_mirror_root() -> PathBuf {
    git_cache().root().to_path_buf()
}

/// The on-disk path of the mirror of `addr`.
pub fn repo_mirror_path(addr: &RepoAddr) -> PathBuf {
    git_cache().repo_path(addr)
}

/// Open the mirror of `addr`, if it has been cloned.
pub fn open_repo_mirror(addr: &RepoAddr) -> Result<Option<Repository>> {
    git_cache().open(addr)
}

/// Open the mirror of `addr`, cloning it first when it does not exist yet.
pub fn ensure_repo_mirror<U: AsRef<str>>(addr: &RepoAddr, clone_urls: &[U]) -> Result<Repository> {
    git_cache().ensure_clone(addr, clone_urls)
}

pub(crate) fn set_git_cache(root: impl Into<PathBuf>) {
    if GIT_CACHE.set(GitCache::new(root.into())).is_err() {
        log::warn!("git cache root is already set, keeping the first one");
    }
}
