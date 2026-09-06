use std::path::PathBuf;

use gpui::{App, Global};
use signed_git::GitCache;

struct GlobalGitStore(GitCache);

impl Global for GlobalGitStore {}

/// Global access to the on-disk git clone cache, the grasp mirrors.
#[derive(Debug, Clone)]
pub struct GitStore(GitCache);

impl GitStore {
    /// Register the clone cache rooted at `root` as an app-wide global.
    pub fn set_global(root: impl Into<PathBuf>, cx: &mut App) -> Self {
        let store = Self::new(root);
        cx.set_global(GlobalGitStore(store.0.clone()));
        store
    }

    /// The app-wide clone cache.
    pub fn global(cx: &App) -> Self {
        Self(cx.global::<GlobalGitStore>().0.clone())
    }

    fn new(root: impl Into<PathBuf>) -> Self {
        Self(GitCache::new(root.into()))
    }

    /// Underlying clone cache.
    pub fn cache(&self) -> &GitCache {
        &self.0
    }
}
