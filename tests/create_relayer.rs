mod common;

use crate::common::prelude::*;

#[tokio::test]
async fn create_relayer() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (_service, client) =
        ServiceBuilder::default().build(&anvil, &db_url).await?;

    let CreateRelayerResponse { .. } = client
        .create_relayer(&CreateRelayerRequest {
            name: "Test relayer".to_string(),
            chain_id: DEFAULT_ANVIL_CHAIN_ID,
        })
        .await?;

    Ok(())
}
