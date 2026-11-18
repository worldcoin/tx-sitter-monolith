mod common;

use tx_sitter_client::apis::relayer_v1_api::CreateTransactionParams;

use crate::common::prelude::*;

#[tokio::test]
async fn worker_broadcasts_txs_sent_through_api() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (worker, _) = ServiceBuilder::default()
        .role(ServiceRole::Worker)
        .build(&anvil, &db_url)
        .await?;
    let (_api, client) = ServiceBuilder::default()
        .role(ServiceRole::Api)
        .build(&anvil, &db_url)
        .await?;

    for path in ["/health", "/health/live", "/health/ready"] {
        let (status, _) = http_get(worker.local_addr(), path).await?;
        assert_eq!(status, StatusCode::OK, "worker {path}");
    }
    for path in ["/2/tx/tx-1", "/swagger", "/schema.json"] {
        let (status, _) = http_get(worker.local_addr(), path).await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "worker {path}");
    }

    let api_key = DEFAULT_API_KEY.to_string();

    let value: U256 = parse_units("1", "ether")?.into();
    tx_sitter_client::apis::relayer_v1_api::create_transaction(
        &client,
        CreateTransactionParams {
            api_token: api_key,
            send_tx_request: SendTxRequest {
                to: ARBITRARY_ADDRESS.into(),
                value: value.into(),
                gas_limit: U256::from(21_000).into(),
                ..Default::default()
            },
        },
    )
    .await?;

    let provider = setup_provider(anvil.endpoint()).await?;
    await_balance(&provider, value, ARBITRARY_ADDRESS).await?;

    Ok(())
}
