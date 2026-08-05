use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use nostr_gossip_memory::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use nostr_lmdb::prelude::*;
#[cfg(target_arch = "wasm32")]
use nostr_memory::prelude::*;
use nostr_sdk::prelude::*;

use crate::signer::UniversalSigner;

/// Owns the nostr client: relay pool, LMDB database and signer.
///
/// The SDK manages its own internal tokio runtime; every method here is a
/// plain async fn that can be driven by GPUI's executors.
#[derive(Clone)]
pub struct NostrBackend {
    client: Client,
    signer: UniversalSigner,
}

impl NostrBackend {
    /// Open (or create) the LMDB database at `db_path` and build the client.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn new(db_path: impl AsRef<std::path::Path>) -> Result<Self> {
        let signer = UniversalSigner::new(Keys::generate());
        let database = NostrLmdb::open(db_path)
            .await
            .context("failed to open nostr database")?;
        Ok(Self::with_database(signer, database))
    }

    /// In-memory database on wasm (no LMDB available).
    #[cfg(target_arch = "wasm32")]
    pub fn new() -> Result<Self> {
        let signer = UniversalSigner::new(Keys::generate());
        Ok(Self::with_database(signer, MemoryDatabase::unbounded()))
    }

    fn with_database<D>(signer: UniversalSigner, database: D) -> Self
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

        Self { client, signer }
    }

    pub fn client(&self) -> Client {
        self.client.clone()
    }

    pub fn signer(&self) -> UniversalSigner {
        self.signer.clone()
    }

    pub async fn add_relay(&self, url: &str) -> Result<()> {
        self.client.add_relay(url).await?;
        Ok(())
    }

    /// Add a relay used only for discovery (e.g. NIP-65 indexer relays).
    /// No subscriptions or writes are routed through it.
    pub async fn add_discovery_relay(&self, url: &str) -> Result<()> {
        self.client
            .add_relay(url)
            .capabilities(RelayCapabilities::DISCOVERY)
            .await?;
        Ok(())
    }

    pub async fn connect(&self) {
        self.client.connect().await;
    }

    /// Start a persistent subscription. Received events are stored in the
    /// database automatically by the relay pool.
    pub async fn subscribe(&self, filter: Filter) -> Result<SubscriptionId> {
        let output = self.client.subscribe(filter).await?;
        Ok(output.value)
    }

    /// Query the local database (the single source of truth for the UI).
    pub async fn query(&self, filter: Filter) -> Result<Vec<Event>> {
        let events = self.client.database().query(filter).await?;
        Ok(events.into_iter().collect())
    }

    /// Sign with the current signer, broadcast, and save locally so the
    /// event is immediately visible to [`NostrBackend::query`].
    pub async fn send(&self, builder: EventBuilder) -> Result<Event> {
        let event = builder.finalize_async(&self.signer).await?;

        let output = self.client.send_event(&event).await?;

        // Keep our own events in the local database; the notification pump
        // only fires for events received from relays.
        self.client.database().save_event(&event).await?;

        if output.success.is_empty() && !output.failed.is_empty() {
            let reasons = output
                .failed
                .values()
                .cloned()
                .collect::<Vec<String>>()
                .join(", ");
            return Err(anyhow!("event not accepted by any relay: {reasons}"));
        }

        Ok(event)
    }
}
