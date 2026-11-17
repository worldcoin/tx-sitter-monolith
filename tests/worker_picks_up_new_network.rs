mod common;

use crate::common::prelude::*;

/// Networks are reconciled every 30 seconds
const NETWORK_PICKUP_TIMEOUT: Duration = Duration::from_secs(60);

#[tokio::test]
async fn worker_indexes_network_added_after_it_started() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (worker, _) = ServiceBuilder::default()
        .role(ServiceRole::Worker)
        .without_defaults()
        .build(&anvil, &db_url)
        .await?;

    connect_db(&db_url)
        .await?
        .upsert_network(
            DEFAULT_ANVIL_CHAIN_ID,
            "Anvil",
            &anvil.endpoint(),
            &anvil.ws_endpoint(),
        )
        .await?;

    await_estimates(&worker, DEFAULT_ANVIL_CHAIN_ID, NETWORK_PICKUP_TIMEOUT)
        .await?;

    Ok(())
}
