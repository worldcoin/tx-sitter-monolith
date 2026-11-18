mod common;

use tx_sitter_client::apis::relayer_v1_api::CreateTransactionParams;

use crate::common::prelude::*;

#[tokio::test]
async fn disabled_relayer() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let relayer = TestRelayer::random(DEFAULT_ANVIL_CHAIN_ID);

    let (_service, client) = ServiceBuilder::default()
        .relayer(RelayerConfig {
            enabled: false,
            ..relayer.config
        })
        .build(&anvil, &db_url)
        .await?;

    let value: U256 = parse_units("1", "ether")?.into();
    let response = tx_sitter_client::apis::relayer_v1_api::create_transaction(
        &client,
        CreateTransactionParams {
            api_token: relayer.api_key,
            send_tx_request: SendTxRequest {
                to: ARBITRARY_ADDRESS.into(),
                value: value.into(),
                gas_limit: U256::from(21_000).into(),
                ..Default::default()
            },
        },
    )
    .await;

    assert!(response.is_err());

    Ok(())
}
