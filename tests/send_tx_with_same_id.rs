mod common;

use poem::http;
use tx_sitter_client::apis::admin_v1_api::RelayerCreateApiKeyParams;
use tx_sitter_client::apis::relayer_v1_api::CreateTransactionParams;
use tx_sitter_client::apis::Error;

use crate::common::prelude::*;

#[tokio::test]
async fn send_tx_with_same_id() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (_service, client) =
        ServiceBuilder::default().build(&anvil, &db_url).await?;
    let CreateApiKeyResponse { api_key } =
        tx_sitter_client::apis::admin_v1_api::relayer_create_api_key(
            &client,
            RelayerCreateApiKeyParams {
                relayer_id: DEFAULT_RELAYER_ID.to_string(),
            },
        )
        .await?;

    let tx_id = Some("tx-1".to_string());

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
                tx_id: tx_id.clone(),
                ..Default::default()
            },
        },
    )
    .await?;

    let res = tx_sitter_client::apis::relayer_v1_api::create_transaction(
        &client,
        CreateTransactionParams {
            api_token: api_key.clone(),
            send_tx_request: SendTxRequest {
                to: ARBITRARY_ADDRESS.into(),
                value: value.into(),
                gas_limit: U256::from(21_000).into(),
                tx_id: tx_id.clone(),
                ..Default::default()
            },
        },
    )
    .await;

    if let Err(Error::ResponseError(e)) = res {
        assert_eq!(e.status, http::StatusCode::CONFLICT);
        assert_eq!(e.content, "Transaction with same id already exists.");

        return Ok(());
    }

    panic!("Should return error on second insert with same id.")
}
