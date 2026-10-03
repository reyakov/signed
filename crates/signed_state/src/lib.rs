mod backend;
mod bootstrap;
mod checkouts;
mod git_store;
mod inbox;
mod local_repos;
mod profile;
mod push;
mod refresh;
mod repo;
mod repos;

use std::path::{Path, PathBuf};

pub use backend::{Backend, BackendEvent};
pub use bootstrap::user_grasp_list_servers;
pub use checkouts::{CheckoutStatus, CheckoutsStore};
pub use git_store::Mirrors;
use gpui::{App, AppContext};
pub use inbox::Inbox;
pub use local_repos::{LocalReposStore, ResolvedLocalRepo};
pub use nostr_sdk::prelude::Timestamp;
pub use profile::{Profile, ProfileStore};
pub use push::{GraspServer, PushOutcome};
pub use refresh::{RefreshGate, RefreshRequest};
pub use repo::RepoStore;
pub use repos::RepoListStore;
pub use signed_git::{Nip34Binding, Nip34Kind};
use signed_nostr::NostrBackend;

pub fn init(
    db_path: impl AsRef<Path>,
    repos_root: impl Into<PathBuf>,
    scan_paths: Vec<PathBuf>,
    cx: &mut App,
) {
    // rustls uses the `aws_lc_rs` provider by default.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let backend = cx
        .foreground_executor()
        .block_on(async move { NostrBackend::open(db_path.as_ref()).await });
    let backend = backend.expect("failed to initialize nostr backend");
    let (client, signer) = (backend.client, backend.signer);

    Mirrors::install(repos_root);

    Backend::set_global(cx.new(|cx| Backend::new(client, signer, cx)), cx);
    ProfileStore::set_global(cx.new(ProfileStore::new), cx);
    RepoListStore::set_global(cx.new(RepoListStore::new), cx);
    LocalReposStore::set_global(cx.new(|cx| LocalReposStore::new(scan_paths, cx)), cx);
    CheckoutsStore::set_global(cx.new(CheckoutsStore::new), cx);
}
