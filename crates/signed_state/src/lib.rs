mod backend;
mod git_store;
mod local_repos;
mod profile;
mod repo;
mod repo_list;

use std::path::{Path, PathBuf};

pub use backend::{Backend, BackendEvent};
pub use git_store::GitStore;
use gpui::{App, AppContext, Entity};
pub use local_repos::LocalReposStore;
pub use nostr_sdk::prelude::Timestamp;
pub use profile::{Profile, ProfileStore};
pub use repo::RepoStore;
pub use repo_list::{RepoActivityCounts, RepoListStore};
use signed_nostr::new_backend;
pub use utils::shorten_pubkey;

/// The default directories scanned for local git repositories
/// on every platform: the user's Desktop and Documents folders.
#[cfg(not(target_arch = "wasm32"))]
fn default_scan_paths() -> Vec<PathBuf> {
    vec![paths::desktop_dir(), paths::documents_dir()]
}

/// Initialize the backend and stores, and install them as globals.
/// Call once at startup, before opening any window that uses the stores.
#[cfg(not(target_arch = "wasm32"))]
pub fn init(db_path: impl AsRef<Path>, cx: &mut App) -> Entity<Backend> {
    // rustls uses the `aws_lc_rs` provider by default; ignore if already installed.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();

    // Initialize the nostr client and universal signer.
    let (client, signer) = cx.foreground_executor().block_on(async move {
        let path = db_path.as_ref().to_path_buf();
        new_backend(path)
            .await
            .expect("failed to initialize nostr backend")
    });

    // Initialize the backend and stores.
    let entity = cx.new(|cx| Backend::new(client, signer, cx));
    Backend::set_global(entity.clone(), cx);

    // Initialize the profile store.
    ProfileStore::set_global(cx.new(ProfileStore::new), cx);

    // Start the explore list from the local database before
    // the first window opens, relay syncs continue in the background,
    // so the list never waits for them.
    RepoListStore::set_global(cx.new(|cx| RepoListStore::new(None, cx)), cx);

    // The clone cache is only meaningful on native platforms,
    // the wasm build registers an empty store so `GitStore::global` still works.
    GitStore::set_global(PathBuf::new(), cx);

    // Scan the default directories (Desktop, Documents) for local git
    // repositories; the sidebar lists them next to the user's NIP-34 repos.
    LocalReposStore::set_global(
        cx.new(|cx| LocalReposStore::new(default_scan_paths(), cx)),
        cx,
    );

    entity
}

/// Initialize the backend with an in-memory database on wasm.
#[cfg(target_arch = "wasm32")]
pub fn init(cx: &mut App) -> Entity<Backend> {
    // Initialize the nostr client and universal signer.
    let (client, signer) = new_backend().expect("failed to initialize nostr backend");

    // Initialize the backend and stores.
    let entity = cx.new(|cx| Backend::new(client, signer, cx));
    Backend::set_global(entity.clone(), cx);

    // Initialize the profile store.
    ProfileStore::set_global(cx.new(ProfileStore::new), cx);

    // Start the explore list from the local database before
    // the first window opens, relay syncs continue in the background,
    // so the list never waits for them.
    RepoListStore::set_global(cx.new(|cx| RepoListStore::new(None, cx)), cx);

    // The clone cache is only meaningful on native platforms,
    // the wasm build registers an empty store so `GitStore::global` still works.
    GitStore::set_global(PathBuf::new(), cx);

    // No filesystem scan on wasm: there are no local git repositories.
    LocalReposStore::set_global(cx.new(|cx| LocalReposStore::new(Vec::new(), cx)), cx);

    entity
}
