use std::path::PathBuf;
use std::sync::OnceLock;

use anyhow::Result;
use nostr::prelude::Url;
use signed_core::RepoAddr;
use signed_git::{GitCache, Repo};

static GIT_CACHE: OnceLock<GitCache> = OnceLock::new();

pub struct Mirrors;

impl Mirrors {
    pub fn install(root: impl Into<PathBuf>) {
        if GIT_CACHE.set(GitCache::new(root.into())).is_err() {
            log::warn!("git cache root is already set, keeping the first one");
        }
    }

    fn cache() -> &'static GitCache {
        GIT_CACHE
            .get()
            .expect("git cache is initialized by signed_state::init")
    }

    pub(crate) fn root() -> PathBuf {
        Self::cache().root().to_path_buf()
    }

    pub fn path(addr: &RepoAddr) -> PathBuf {
        Self::cache().repo_path(addr)
    }

    pub fn open(addr: &RepoAddr) -> Result<Option<Repo>> {
        Self::cache().open(addr)
    }

    pub fn ensure(addr: &RepoAddr, clone_urls: &[Url]) -> Result<Repo> {
        Self::cache().ensure_clone(addr, clone_urls)
    }
}
