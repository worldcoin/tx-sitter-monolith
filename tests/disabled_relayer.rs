mod common;

use tx_sitter::types::RelayerUpdate;

use crate::common::prelude::*;

#[tokio::test]
async fn disabled_relayer() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (_service, client) =
        ServiceBuilder::default().build(&anvil, &db_url).await?;

    tracing::info!("Creating relayer");
    let CreateRelayerResponse { relayer_id, .. } = client
        .create_relayer(&CreateRelayerRequest {
            name: "Test relayer".to_string(),
            chain_id: DEFAULT_ANVIL_CHAIN_ID,
        })
        .await?;

    tracing::info!("Creating API key");
    let CreateApiKeyResponse { api_key } =
        client.create_relayer_api_key(&relayer_id).await?;

    tracing::info!("Disabling relayer");
    client
        .update_relayer(
            &relayer_id,
            RelayerUpdate::default().with_enabled(false),
        )
        .await?;

    let value: U256 = parse_units("1", "ether")?.into();
    let response = client
        .send_tx(
            &api_key,
            &SendTxRequest {
                to: ARBITRARY_ADDRESS,
                value,
                gas_limit: U256::from(21_000),
                ..Default::default()
            },
        )
        .await;

    assert!(response.is_err());

    Ok(())
}
