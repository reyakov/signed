use std::collections::HashMap;
use std::time::Duration;

use anyhow::Error;
use nostr_connect::prelude::*;
use nostr_sdk::client::SyncSummary;
use nostr_sdk::prelude::*;
use signed_core::Filters;

pub const BOOTSTRAP_RELAYS: [&str; 2] = ["wss://relay.ditto.pub", "wss://index.ngit.dev"];
pub const INDEXER_RELAYS: [&str; 2] = ["wss://indexer.coracle.social", "wss://user.kindpag.es"];

pub(crate) async fn ensure_bootstrap_relays(client: &Client) -> Result<(), Error> {
    for url in BOOTSTRAP_RELAYS {
        client.add_relay(url).and_connect().await?;
    }

    for url in INDEXER_RELAYS {
        client
            .add_relay(url)
            .capabilities(RelayCapabilities::DISCOVERY)
            .and_connect()
            .await?;
    }

    Ok(())
}

pub(crate) async fn subscribe_bootstrap_only(
    client: &Client,
    filters: Vec<Filter>,
) -> Result<(), Error> {
    ensure_bootstrap_relays(client).await?;

    let opts = SubscribeAutoCloseOptions::default()
        .exit_policy(ReqExitPolicy::ExitOnEOSE)
        .timeout(Some(Duration::from_secs(10)));

    let target: HashMap<&str, Vec<Filter>> = BOOTSTRAP_RELAYS
        .iter()
        .map(|relay| (*relay, filters.clone()))
        .collect();

    client.subscribe(target).close_on(opts).await?;

    Ok(())
}

pub(crate) async fn sync_bootstrap_only(
    client: &Client,
    filter: Filter,
    opts: SyncOptions,
) -> Result<SyncSummary, Error> {
    ensure_bootstrap_relays(client).await?;

    let output = client
        .sync(filter)
        .with(BOOTSTRAP_RELAYS)
        .opts(opts)
        .await?;

    Ok(output.value)
}

fn grasp_list_servers(event: &Event) -> Vec<RelayUrl> {
    event
        .tags
        .iter()
        .filter(|tag| tag.kind() == "g")
        .filter_map(|tag| tag.content())
        .filter_map(|url| RelayUrl::parse(url).ok())
        .collect()
}

pub async fn user_grasp_list_servers(
    client: &Client,
    user: PublicKey,
) -> Result<Vec<RelayUrl>, Error> {
    let events: Vec<Event> = client
        .database()
        .query(Filters::grasp_list(user))
        .await?
        .into_iter()
        .collect();

    let latest = events
        .into_iter()
        .max_by_key(|event| event.created_at)
        .map(|event| grasp_list_servers(&event))
        .unwrap_or_default();

    Ok(latest)
}
