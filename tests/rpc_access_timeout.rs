mod common;

use std::time::Instant;

use eyre::Context;
use poem::http;
use tx_sitter_client::apis::admin_v1_api::{
    CreateNetworkParams, CreateRelayerParams, RelayerCreateApiKeyParams,
};
use tx_sitter_client::apis::relayer_v1_api::CallRpcParams;
use tx_sitter_client::apis::Error;

use crate::common::prelude::*;

const RPC_TIMEOUT: Duration = Duration::from_secs(2);
const UNRESPONSIVE_CHAIN_ID: i32 = 1337;

#[tokio::test]
async fn rpc_access_fails_within_rpc_timeout() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (_service, client) = ServiceBuilder::default()
        .rpc_timeout(RPC_TIMEOUT)
        .build(&anvil, &db_url)
        .await?;

    let rpc_addr = spawn_unresponsive_rpc().await?;
    tx_sitter_client::apis::admin_v1_api::create_network(
        &client,
        CreateNetworkParams {
            chain_id: UNRESPONSIVE_CHAIN_ID,
            create_network_request: CreateNetworkRequest::new(
                "Unresponsive".to_string(),
                format!("http://{rpc_addr}"),
                format!("ws://{rpc_addr}"),
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
                    UNRESPONSIVE_CHAIN_ID,
                ),
            },
        )
        .await?;
    let CreateApiKeyResponse { api_key } =
        tx_sitter_client::apis::admin_v1_api::relayer_create_api_key(
            &client,
            RelayerCreateApiKeyParams { relayer_id },
        )
        .await?;

    let started = Instant::now();
    let response = tokio::time::timeout(
        RPC_TIMEOUT * 3,
        tx_sitter_client::apis::relayer_v1_api::call_rpc(
            &client,
            CallRpcParams {
                api_token: api_key,
                rpc_request: RpcRequest::new(
                    1,
                    "eth_blockNumber".to_string(),
                    JsonRpcVersion::Variant2Period0,
                ),
            },
        ),
    )
    .await
    .context("RPC proxy outlived rpc_timeout")?;
    let elapsed = started.elapsed();

    let Err(Error::ResponseError(response)) = response else {
        eyre::bail!("Expected an error response, got {response:?}");
    };
    assert_eq!(response.status, http::StatusCode::INTERNAL_SERVER_ERROR);
    assert!(elapsed >= RPC_TIMEOUT, "Took {elapsed:?}");
    assert!(elapsed <= RPC_TIMEOUT * 2, "Took {elapsed:?}");

    Ok(())
}
