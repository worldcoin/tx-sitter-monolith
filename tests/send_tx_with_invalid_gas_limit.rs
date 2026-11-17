mod common;

use poem::http;
use tx_sitter_client::apis::relayer_v1_api::{
    CreateTransactionParams, GetTransactionsParams,
};
use tx_sitter_client::apis::Error;

use crate::common::prelude::*;

/// A chain whose RPC never answers, so it never gets fee estimates and
/// tx-sitter skips the balance check.
const UNINDEXED_CHAIN_ID: u64 = 1337;
const UNREACHABLE_RPC: &str = "127.0.0.1:1";

#[tokio::test]
async fn send_tx_with_gas_limit_above_u64_max() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let relayer = TestRelayer::random(UNINDEXED_CHAIN_ID);
    let relayer_id = relayer.config.id.clone();
    let api_key = relayer.api_key;

    let (_service, client) = ServiceBuilder::default()
        .network(NetworkConfig {
            chain_id: UNINDEXED_CHAIN_ID,
            name: "Unindexed".to_string(),
            http_rpcs: vec![rpc_url(&format!("http://{UNREACHABLE_RPC}"))],
            ws_rpc: rpc_url(&format!("ws://{UNREACHABLE_RPC}")),
            op_stack: None,
        })
        .relayer(relayer.config)
        .build(&anvil, &db_url)
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

    let relayer = connect_db(&db_url)
        .await?
        .get_relayer(&relayer_id)
        .await?
        .ok_or_else(|| eyre::eyre!("Relayer was not stored"))?;
    assert_eq!(relayer.nonce, 0);

    Ok(())
}
