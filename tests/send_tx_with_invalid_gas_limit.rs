mod common;

use poem::http;
use tx_sitter_client::apis::admin_v1_api::{
    CreateNetworkParams, CreateRelayerParams, GetRelayerParams,
    RelayerCreateApiKeyParams,
};
use tx_sitter_client::apis::relayer_v1_api::{
    CreateTransactionParams, GetTransactionsParams,
};
use tx_sitter_client::apis::Error;

use crate::common::prelude::*;

/// A chain whose RPC never answers, so it never gets fee estimates and
/// tx-sitter skips the balance check.
const UNINDEXED_CHAIN_ID: i32 = 1337;
const UNREACHABLE_RPC: &str = "127.0.0.1:1";

#[tokio::test]
async fn send_tx_with_gas_limit_above_u64_max() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (_service, client) =
        ServiceBuilder::default().build(&anvil, &db_url).await?;

    tx_sitter_client::apis::admin_v1_api::create_network(
        &client,
        CreateNetworkParams {
            chain_id: UNINDEXED_CHAIN_ID,
            create_network_request: CreateNetworkRequest::new(
                "Unindexed".to_string(),
                format!("http://{UNREACHABLE_RPC}"),
                format!("ws://{UNREACHABLE_RPC}"),
            ),
        },
    )
    .await?;

    let CreateRelayerResponse { relayer_id, .. } =
        tx_sitter_client::apis::admin_v1_api::create_relayer(
            &client,
            CreateRelayerParams {
                create_relayer_request: CreateRelayerRequest::new(
                    "Test relayer".to_string(),
                    UNINDEXED_CHAIN_ID,
                ),
            },
        )
        .await?;
    let CreateApiKeyResponse { api_key } =
        tx_sitter_client::apis::admin_v1_api::relayer_create_api_key(
            &client,
            RelayerCreateApiKeyParams {
                relayer_id: relayer_id.clone(),
            },
        )
        .await?;

    let response = tx_sitter_client::apis::relayer_v1_api::create_transaction(
        &client,
        CreateTransactionParams {
            api_token: api_key.clone(),
            send_tx_request: SendTxRequest {
                to: ARBITRARY_ADDRESS.into(),
                value: U256::ZERO.into(),
                gas_limit: (U256::from(u64::MAX) + U256::from(1)).into(),
                ..Default::default()
            },
        },
    )
    .await;

    let Err(Error::ResponseError(response)) = response else {
        eyre::bail!("Expected an error response, got {response:?}");
    };
    assert_eq!(response.status, http::StatusCode::BAD_REQUEST);

    let txs = tx_sitter_client::apis::relayer_v1_api::get_transactions(
        &client,
        GetTransactionsParams {
            api_token: api_key,
            status: None,
            unsent: None,
        },
    )
    .await?;
    assert!(txs.is_empty(), "Transactions were created: {txs:?}");

    let relayer = tx_sitter_client::apis::admin_v1_api::get_relayer(
        &client,
        GetRelayerParams { relayer_id },
    )
    .await?;
    assert_eq!(relayer.nonce, 0);

    Ok(())
}
