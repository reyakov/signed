use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Error;
use gpui::{App, AppContext, Context, Entity, Global, Task};
use signed_git::find_git_repos;

struct GlobalLocalReposStore(Entity<LocalReposStore>);

impl Global for GlobalLocalReposStore {}

/// Store of the git repositories discovered under a set of scan paths.
pub struct LocalReposStore {
    /// The directories being scanned.
    pub roots: Arc<Vec<PathBuf>>,
    /// Git repositories discovered under [`Self::roots`], sorted by path.
    pub repos: Arc<Vec<PathBuf>>,
    /// A scan is currently running.
    pub scanning: bool,
    /// A scan was requested while one was already running.
    scan_dirty: bool,
    tasks: Vec<Task<Result<(), Error>>>,
}

impl LocalReposStore {
    /// Retrieve the global local-repositories store.
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalLocalReposStore>().0.clone()
    }

    pub(crate) fn set_global(entity: Entity<Self>, cx: &mut App) {
        cx.set_global(GlobalLocalReposStore(entity));
    }

    /// Create a store scanning `roots` right away.
    pub fn new(roots: Vec<PathBuf>, cx: &mut Context<Self>) -> Self {
        let mut store = Self {
            roots: Arc::new(roots),
            repos: Arc::new(Vec::new()),
            scanning: false,
            scan_dirty: false,
            tasks: Vec::new(),
        };
        store.rescan(cx);
        store
    }

    /// Forget a repository that has just been published to NIP-34,
    /// so it leaves the local list immediately. A later rescan re-discovers it from disk,
    /// the sidebar additionally hides published repositories by identifier.
    pub fn remove(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.repos = Arc::new(
            self.repos
                .iter()
                .filter(|repo| repo.as_path() != path)
                .cloned()
                .collect(),
        );
        cx.notify();
    }

    /// Re-run the scan.
    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        if self.scanning {
            self.scan_dirty = true;
            return;
        }
        if self.roots.is_empty() {
            return;
        }

        self.scanning = true;
        cx.notify();

        let roots = self.roots.clone();
        let work = cx.background_spawn(async move {
            let mut repos = Vec::new();
            for root in roots.iter() {
                repos.extend(find_git_repos(root));
            }
            repos.sort();
            repos.dedup();
            repos
        });

        self.tasks.push(cx.spawn(async move |this, cx| {
            let repos = work.await;
            let again = this.update(cx, |this, cx| {
                this.repos = Arc::new(repos);
                this.scanning = false;
                cx.notify();

                let dirty = this.scan_dirty;
                this.scan_dirty = false;
                dirty
            })?;

            // Scans requested while this one was running are coalesced into
            // a single follow-up scan.
            if again {
                this.update(cx, |this, cx| this.rescan(cx))?;
            }

            Ok(())
        }));
    }
}
