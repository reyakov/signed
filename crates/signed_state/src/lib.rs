mod backend;
mod git_store;
mod profile;
mod repo;
mod repo_list;

use std::path::{Path, PathBuf};

pub use backend::{Backend, BackendEvent};
pub use git_store::GitStore;
use gpui::{App, AppContext, Entity};
pub use nostr_sdk::prelude::Timestamp;
pub use profile::{Profile, ProfileStore};
pub use repo::RepoStore;
pub use repo_list::{RepoActivityCounts, RepoListStore};
use signed_nostr::new_backend;
pub use utils::shorten_pubkey;

/// Initialize the backend and stores, and install them as globals. Call once
/// at startup, before opening any window that uses the stores.
#[cfg(not(target_arch = "wasm32"))]
pub fn init(db_path: impl AsRef<Path>, cx: &mut App) -> Entity<Backend> {
    // rustls uses the `aws_lc_rs` provider by default; ignore if already installed.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();

    let path = db_path.as_ref().to_path_buf();
    let (client, signer) = cx.foreground_executor().block_on(async move {
        new_backend(path)
            .await
            .expect("failed to initialize nostr backend")
    });

    let entity = cx.new(|cx| Backend::new(client, signer, cx));
    Backend::set_global(entity.clone(), cx);

    ProfileStore::set_global(cx.new(ProfileStore::new), cx);

    // Start the explore list from the local database before the first
    // window opens; relay syncs continue in the background, so the list
    // never waits for them.
    RepoListStore::set_global(cx.new(|cx| RepoListStore::new(None, cx)), cx);

    // The clone cache is only meaningful on native platforms; the wasm
    // build registers an empty store so `GitStore::global` still works.
    GitStore::set_global(PathBuf::new(), cx);

    entity
}

/// Initialize the backend with an in-memory database on wasm.
#[cfg(target_arch = "wasm32")]
pub fn init(cx: &mut App) -> Entity<Backend> {
    let (client, signer) = new_backend().expect("failed to initialize nostr backend");

    let entity = cx.new(|cx| Backend::new(client, signer, cx));
    Backend::set_global(entity.clone(), cx);

    ProfileStore::set_global(cx.new(ProfileStore::new), cx);

    // Start the explore list from the local database before the first
    // window opens; relay syncs continue in the background, so the list
    // never waits for them.
    RepoListStore::set_global(cx.new(|cx| RepoListStore::new(None, cx)), cx);

    GitStore::set_global(PathBuf::new(), cx);

    entity
}
