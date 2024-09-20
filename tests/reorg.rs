mod common;

use tx_sitter_client::apis::admin_v1_api::RelayerCreateApiKeyParams;
use tx_sitter_client::apis::relayer_v1_api::CreateTransactionParams;

use crate::common::prelude::*;

#[tokio::test]
async fn reorg() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let anvil_port = anvil.port();

    let (_service, client) = ServiceBuilder::default()
        .hard_reorg_interval(Duration::from_secs(2))
        .build(&anvil, &db_url)
        .await?;

    let CreateApiKeyResponse { api_key } =
        tx_sitter_client::apis::admin_v1_api::relayer_create_api_key(
            &client,
            RelayerCreateApiKeyParams {
                relayer_id: DEFAULT_RELAYER_ID.to_string(),
            },
        )
        .await?;

    let provider = setup_provider(anvil.endpoint()).await?;

    // Send a transaction
    let value: U256 = parse_units("1", "ether")?.into();
    tx_sitter_client::apis::relayer_v1_api::create_transaction(
        &client,
        CreateTransactionParams {
            api_token: api_key.clone(),
            send_tx_request: SendTxRequest {
                to: ARBITRARY_ADDRESS.into(),
                value: value.into(),
                gas_limit: U256::from(21_000).into(),
                ..Default::default()
            },
        },
    )
    .await?;

    await_balance(&provider, value, ARBITRARY_ADDRESS).await?;

    // Drop anvil to simulate a reorg
    tracing::warn!("Dropping anvil & restarting at port {anvil_port}");
    drop(anvil);

    let anvil = AnvilBuilder::default().port(anvil_port).spawn().await?;
    let provider = setup_provider(anvil.endpoint()).await?;

    await_balance(&provider, value, ARBITRARY_ADDRESS).await?;

    Ok(())
}
