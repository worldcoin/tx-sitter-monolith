mod common;

use std::time::Instant;

use eyre::Context;
use poem::http;
use tx_sitter_client::apis::relayer_v1_api::CallRpcParams;
use tx_sitter_client::apis::Error;

use crate::common::prelude::*;

const RPC_TIMEOUT: Duration = Duration::from_secs(2);
const UNRESPONSIVE_CHAIN_ID: u64 = 1337;

#[tokio::test]
async fn rpc_access_fails_within_rpc_timeout() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let rpc = UnresponsiveRpc::spawn().await?;
    let relayer = TestRelayer::random(UNRESPONSIVE_CHAIN_ID);

    let (_service, client) = ServiceBuilder::default()
        .rpc_timeout(RPC_TIMEOUT)
        .network(rpc.network(UNRESPONSIVE_CHAIN_ID))
        .relayer(relayer.config)
        .build(&anvil, &db_url)
        .await?;

    let started = Instant::now();
    let response = tokio::time::timeout(
        RPC_TIMEOUT * 3,
        tx_sitter_client::apis::relayer_v1_api::call_rpc(
            &client,
            CallRpcParams {
                api_token: relayer.api_key,
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
    assert!(
        !response.content.contains(&rpc.addr.to_string()),
        "Upstream RPC URL leaked in {:?}",
        response.content
    );
    assert!(elapsed >= RPC_TIMEOUT, "Took {elapsed:?}");
    assert!(elapsed <= RPC_TIMEOUT * 2, "Took {elapsed:?}");

    Ok(())
}
