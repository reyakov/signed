mod backend;
mod checkouts;
mod git_store;
mod local_repos;
mod profile;
mod refresh;
mod repo;
mod repo_list;

use std::path::{Path, PathBuf};

pub use backend::{Backend, BackendEvent, user_grasp_list_servers};
pub use checkouts::{CheckoutStatus, CheckoutsStore, pr_proposes_checkout};
pub use git_store::GitStore;
use gpui::{App, AppContext, Entity};
pub use local_repos::LocalReposStore;
pub use nostr_sdk::prelude::Timestamp;
pub use profile::{Profile, ProfileStore};
pub use repo::RepoStore;
pub use repo_list::{RepoActivityCounts, RepoListStore};
use signed_nostr::new_backend;

/// Initialize the backend and stores, and install them as globals.
/// Call once at startup, before opening any window that uses the stores.
#[cfg(not(target_arch = "wasm32"))]
pub fn init(
    db_path: impl AsRef<Path>,
    repos_root: impl Into<PathBuf>,
    scan_paths: Vec<PathBuf>,
    cx: &mut App,
) -> Entity<Backend> {
    // rustls uses the `aws_lc_rs` provider by default.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let (client, signer) = cx.foreground_executor().block_on(async move {
        let path = db_path.as_ref().to_path_buf();
        new_backend(path)
            .await
            .expect("failed to initialize nostr backend")
    });

    let entity = cx.new(|cx| Backend::new(client, signer, cx));
    Backend::set_global(entity.clone(), cx);
    ProfileStore::set_global(cx.new(ProfileStore::new), cx);
    RepoListStore::set_global(cx.new(RepoListStore::new), cx);
    GitStore::set_global(repos_root, cx);
    LocalReposStore::set_global(cx.new(|cx| LocalReposStore::new(scan_paths, cx)), cx);
    CheckoutsStore::set_global(cx.new(CheckoutsStore::new), cx);

    entity
}

/// Initialize the backend with an in-memory database on wasm.
#[cfg(target_arch = "wasm32")]
pub fn init(cx: &mut App) -> Entity<Backend> {
    let (client, signer) = new_backend().expect("failed to initialize nostr backend");
    let entity = cx.new(|cx| Backend::new(client, signer, cx));
    Backend::set_global(entity.clone(), cx);
    ProfileStore::set_global(cx.new(ProfileStore::new), cx);
    RepoListStore::set_global(cx.new(RepoListStore::new), cx);
    GitStore::set_global(PathBuf::new(), cx);
    LocalReposStore::set_global(cx.new(|cx| LocalReposStore::new(Vec::new(), cx)), cx);
    CheckoutsStore::set_global(cx.new(|cx| CheckoutsStore::new(cx)), cx);
    entity
}
