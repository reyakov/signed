mod backend;
mod bootstrap;
mod checkouts;
mod git_store;
mod inbox;
mod local_repos;
mod patch_series;
mod push;
mod refresh;
mod repo;
mod repos;
mod state;
mod sync_status;

use std::path::{Path, PathBuf};

pub use backend::{Backend, BackendEvent};
pub use bootstrap::{sync_bootstrap_only, user_grasp_list_servers};
pub use checkouts::CheckoutsStore;
pub use git_store::Mirrors;
use gpui::{App, AppContext};
pub use inbox::Inbox;
pub use local_repos::{LocalReposStore, ResolvedLocalRepo};
pub use nostr_sdk::prelude::Timestamp;
pub use push::{GraspServer, PushOutcome};
pub use refresh::{RefreshGate, RefreshRequest};
pub use repo::RepoStore;
pub use repos::RepoListStore;
pub use signed_git::{Nip34Binding, Nip34Kind};
use signed_nostr::NostrBackend;
pub use state::{GitPushMode, GitPushPolicy, GitRef, PushStep, PushStepStatus, SyncState};
pub use sync_status::{CheckoutSyncStatus, SyncEvent, SyncStatusDirectory, SyncStatusStore};

pub fn init(
    db_path: impl AsRef<Path>,
    repos_root: impl Into<PathBuf>,
    scan_paths: Vec<PathBuf>,
    cx: &mut App,
) -> anyhow::Result<()> {
    // rustls uses the `aws_lc_rs` provider by default.
    if rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .is_err()
    {
        log::debug!("a crypto provider is already installed");
    }

    let db_path = db_path.as_ref().to_path_buf();

    let backend = match cx
        .foreground_executor()
        .block_on(NostrBackend::open(&db_path))
    {
        Ok(backend) => backend,
        Err(error) => {
            // A damaged or locked database must not keep the app from starting.
            log::error!(
                "failed to open the nostr database {}: {error:#}",
                db_path.display()
            );

            let fallback = std::env::temp_dir()
                .join("signed")
                .join(format!("nostr-fallback-{}", std::process::id()));

            let backend = std::fs::create_dir_all(&fallback)
                .map_err(anyhow::Error::from)
                .and_then(|()| {
                    cx.foreground_executor()
                        .block_on(NostrBackend::open(&fallback))
                });

            if backend.is_ok() {
                cx.show_system_notification(gpui::SystemNotification {
                    tag: "signed-nostr-db".into(),
                    title: "Nostr cache unavailable".into(),
                    body: format!(
                        "{} could not be opened; this session uses a temporary cache",
                        db_path.display()
                    )
                    .into(),
                    actions: Vec::new(),
                });
            }

            backend?
        }
    };

    let (client, signer) = (backend.client, backend.signer);

    Mirrors::install(repos_root);

    Backend::set_global(cx.new(|cx| Backend::new(client, signer, cx)), cx);
    RepoListStore::set_global(cx.new(RepoListStore::new), cx);
    LocalReposStore::set_global(cx.new(|cx| LocalReposStore::new(scan_paths, cx)), cx);
    CheckoutsStore::set_global(cx.new(CheckoutsStore::new), cx);
    SyncStatusStore::set_global(cx.new(SyncStatusStore::new), cx);

    Ok(())
}
