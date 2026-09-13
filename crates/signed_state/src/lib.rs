mod backend;
mod checkouts;
mod git_store;
mod inbox;
mod profile;
mod refresh;
mod repo;
mod repos;

use std::path::{Path, PathBuf};

pub use backend::{Backend, BackendEvent, user_grasp_list_servers};
pub use checkouts::{CheckoutStatus, CheckoutsStore, pr_proposes_checkout};
pub use git_store::GitStore;
use gpui::{App, AppContext};
pub use inbox::{Inbox, query_inbox};
pub use nostr_sdk::prelude::Timestamp;
pub use profile::{Profile, ProfileStore};
pub use refresh::{RefreshGate, RefreshRequest};
pub use repo::RepoStore;
pub use repos::{LocalReposStore, RepoActivityCounts, RepoListStore};
use signed_nostr::new_backend;

#[cfg(not(target_arch = "wasm32"))]
pub fn init(
    db_path: impl AsRef<Path>,
    repos_root: impl Into<PathBuf>,
    scan_paths: Vec<PathBuf>,
    cx: &mut App,
) {
    // rustls uses the `aws_lc_rs` provider by default.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let (client, signer) = cx.foreground_executor().block_on(async move {
        let path = db_path.as_ref().to_path_buf();
        new_backend(path)
            .await
            .expect("failed to initialize nostr backend")
    });

    Backend::set_global(cx.new(|cx| Backend::new(client, signer, cx)), cx);
    ProfileStore::set_global(cx.new(ProfileStore::new), cx);
    RepoListStore::set_global(cx.new(RepoListStore::new), cx);
    GitStore::set_global(repos_root, cx);
    LocalReposStore::set_global(cx.new(|cx| LocalReposStore::new(scan_paths, cx)), cx);
    CheckoutsStore::set_global(cx.new(CheckoutsStore::new), cx);
}

/// Initialize the backend with an in-memory database on wasm.
#[cfg(target_arch = "wasm32")]
pub fn init(cx: &mut App) {
    let (client, signer) = new_backend().expect("failed to initialize nostr backend");
    Backend::set_global(cx.new(|cx| Backend::new(client, signer, cx)), cx);
    ProfileStore::set_global(cx.new(ProfileStore::new), cx);
    RepoListStore::set_global(cx.new(RepoListStore::new), cx);
    GitStore::set_global(PathBuf::new(), cx);
    LocalReposStore::set_global(cx.new(|cx| LocalReposStore::new(Vec::new(), cx)), cx);
    CheckoutsStore::set_global(cx.new(|cx| CheckoutsStore::new(cx)), cx);
}
