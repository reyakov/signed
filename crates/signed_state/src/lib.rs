mod backend;
mod profile;
mod repo;
mod repo_list;

use std::path::Path;

pub use backend::{Backend, BackendEvent};
use gpui::{App, AppContext, Entity};
pub use profile::{Profile, ProfileStore, shorten_pubkey};
pub use repo::RepoStore;
pub use repo_list::RepoListStore;
use signed_nostr::NostrBackend;

/// Initialize the backend and stores, and install them as globals. Call once
/// at startup, before opening any window that uses the stores.
#[cfg(not(target_arch = "wasm32"))]
pub fn init(db_path: impl AsRef<Path>, cx: &mut App) -> Entity<Backend> {
    // rustls uses the `aws_lc_rs` provider by default; ignore if already installed.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();

    let path = db_path.as_ref().to_path_buf();
    let inner = cx.foreground_executor().block_on(async move {
        NostrBackend::new(path)
            .await
            .expect("failed to initialize nostr backend")
    });

    let entity = cx.new(|cx| Backend::new(inner, cx));
    Backend::set_global(entity.clone(), cx);

    ProfileStore::set_global(cx.new(ProfileStore::new), cx);

    entity
}

/// Initialize the backend with an in-memory database on wasm.
#[cfg(target_arch = "wasm32")]
pub fn init(cx: &mut App) -> Entity<Backend> {
    let inner = NostrBackend::new().expect("failed to initialize nostr backend");

    let entity = cx.new(|cx| Backend::new(inner, cx));
    Backend::set_global(entity.clone(), cx);

    ProfileStore::set_global(cx.new(ProfileStore::new), cx);

    entity
}
