#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use nostr_gossip_memory::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use nostr_lmdb::prelude::*;
#[cfg(target_arch = "wasm32")]
use nostr_memory::prelude::*;
use nostr_sdk::prelude::*;

use crate::signer::UniversalSigner;

/// Open (or create) the LMDB database at `db_path` and build a client
/// configured for Signed, together with a fresh signer.
///
/// The SDK manages its own internal tokio runtime; the returned client can be
/// driven by GPUI's executors.
#[cfg(not(target_arch = "wasm32"))]
pub async fn new_backend(db_path: impl AsRef<Path>) -> Result<(Client, UniversalSigner)> {
    let signer = UniversalSigner::new(Keys::generate());
    let database = NostrLmdb::open(db_path)
        .await
        .context("failed to open nostr database")?;
    Ok(with_database(signer, database))
}

/// In-memory database on wasm (no LMDB available).
#[cfg(target_arch = "wasm32")]
pub fn new_backend() -> Result<(Client, UniversalSigner)> {
    let signer = UniversalSigner::new(Keys::generate());
    Ok(with_database(signer, MemoryDatabase::unbounded()))
}

fn with_database<D>(signer: UniversalSigner, database: D) -> (Client, UniversalSigner)
where
    D: IntoNostrDatabase,
{
    let authenticator = SignerAuthenticator::new(signer.clone());

    let client = ClientBuilder::default()
        .database(database)
        .authenticator(authenticator)
        .gossip(NostrGossipMemory::unbounded())
        .gossip_config(GossipConfig::default().no_background_refresh())
        .connect_timeout(Duration::from_secs(10))
        .verify_subscriptions(true)
        .ban_relay_on_mismatch(true)
        .sleep_when_idle(SleepWhenIdle::Enabled {
            timeout: Duration::from_secs(600),
        })
        .build();

    (client, signer)
}
