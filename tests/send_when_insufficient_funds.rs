mod common;

use poem::http;
use tx_sitter_client::apis::admin_v1_api::RelayerCreateApiKeyParams;
use tx_sitter_client::apis::relayer_v1_api::CreateTransactionParams;
use tx_sitter_client::apis::Error;

use crate::common::prelude::*;

const ESCALATION_INTERVAL: Duration = Duration::from_secs(2);
const ANVIL_BLOCK_TIME: u64 = 6;

#[tokio::test]
async fn send_when_insufficient_funds() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default()
        .block_time(ANVIL_BLOCK_TIME)
        .spawn()
        .await?;

    let (_service, client) = ServiceBuilder::default()
        .escalation_interval(ESCALATION_INTERVAL)
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
    let value: U256 = parse_units("9999.9999", "ether")?.into();

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

    for _ in 0..5 {
        let tx = tx_sitter_client::apis::relayer_v1_api::create_transaction(
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

        if let Err(Error::ResponseError(e)) = tx {
            assert_eq!(e.status, http::StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(
                e.content,
                "Relayer funds are insufficient for transaction to be mined."
            );
            return Ok(());
        }
    }

    eyre::bail!("Should return error response with information about insufficient funds.")
}
