use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use nostr_gossip_memory::prelude::*;
use nostr_lmdb::prelude::*;
use nostr_sdk::prelude::*;

use crate::signer::UniversalSigner;

pub struct NostrBackend {
    pub client: Client,
    pub signer: UniversalSigner,
}

impl NostrBackend {
    pub async fn open(db_path: impl AsRef<Path>) -> Result<Self> {
        let db_path = db_path.as_ref();
        let signer = UniversalSigner::new(Keys::generate());
        let database = NostrLmdb::open(db_path)
            .await
            .context("failed to open nostr database")?;
        Ok(Self::with_database(signer, database))
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
}
