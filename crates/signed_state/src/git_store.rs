use std::path::PathBuf;

use gpui::{App, Global};
use signed_git::GitCache;

struct GlobalGitStore(GitCache);

impl Global for GlobalGitStore {}

/// Global access to the on-disk git clone cache (grasp mirrors).
///
/// Installed at startup via [`GitStore::set_global`]; see also
/// [`signed_state::init`].
#[derive(Debug, Clone)]
pub struct GitStore(GitCache);

impl GitStore {
    /// Register the clone cache rooted at `root` as an app-wide global.
    /// Replaces any previously installed store (see [`signed_state::init`], which
    /// installs an empty one).
    pub fn set_global(root: impl Into<PathBuf>, cx: &mut App) -> Self {
        let store = Self::new(root);
        cx.set_global(GlobalGitStore(store.0.clone()));
        store
    }

    /// The app-wide clone cache.
    ///
    /// # Panics
    ///
    /// Panics if [`GitStore::set_global`] was never called.
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
