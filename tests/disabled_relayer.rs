mod common;

use tx_sitter_client::apis::admin_v1_api::{
    CreateRelayerParams, RelayerCreateApiKeyParams, UpdateRelayerParams,
};
use tx_sitter_client::apis::relayer_v1_api::CreateTransactionParams;

use crate::common::prelude::*;

#[tokio::test]
async fn disabled_relayer() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (_service, client) =
        ServiceBuilder::default().build(&anvil, &db_url).await?;

    tracing::info!("Creating relayer");
    let CreateRelayerResponse { relayer_id, .. } =
        tx_sitter_client::apis::admin_v1_api::create_relayer(
            &client,
            CreateRelayerParams {
                create_relayer_request: CreateRelayerRequest::new(
                    "Test relayer".to_string(),
                    DEFAULT_ANVIL_CHAIN_ID as i32,
                ),
            },
        )
        .await?;

    tracing::info!("Creating API key");
    let CreateApiKeyResponse { api_key } =
        tx_sitter_client::apis::admin_v1_api::relayer_create_api_key(
            &client,
            RelayerCreateApiKeyParams {
                relayer_id: relayer_id.clone(),
            },
        )
        .await?;

    tracing::info!("Disabling relayer");
    tx_sitter_client::apis::admin_v1_api::update_relayer(
        &client,
        UpdateRelayerParams {
            relayer_id: relayer_id.clone(),
            relayer_update: RelayerUpdate {
                enabled: Some(false),
                ..Default::default()
            },
        },
    )
    .await?;

    let value: U256 = parse_units("1", "ether")?.into();
    let response = tx_sitter_client::apis::relayer_v1_api::create_transaction(
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
    .await;

    assert!(response.is_err());

    Ok(())
}
