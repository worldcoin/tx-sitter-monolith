mod common;

use alloy::network::TransactionBuilder;
use alloy::node_bindings::AnvilInstance;
use alloy::providers::ProviderBuilder;
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use tx_sitter::types::ErrorResponseBody;
use tx_sitter_client::apis::relayer_v1_api::CreateTransactionParams;
use tx_sitter_client::apis::relayer_v2_api::{
    CreateTransactionV2Params, GetTransactionV2Params,
};
use tx_sitter_client::apis::Error;

use crate::common::prelude::*;

/// How many transactions the key sends before it joins tx-sitter
const PRIOR_TXS: u64 = 3;

/// A key that already sent transactions elsewhere, like one imported from
/// tx-sitter-aws, joins as a new relayer. The API takes no transactions for
/// it until a worker syncs its nonce from the chain, and its first
/// transaction then takes the chain's nonce.
#[tokio::test]
async fn imported_key_continues_from_the_chain_nonce() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let key = PrivateKeySigner::from_slice(&anvil.keys()[2].to_bytes())?;
    send_txs_outside_tx_sitter(&anvil, key.clone(), PRIOR_TXS).await?;
    let relayer =
        TestRelayer::with_key(DEFAULT_ANVIL_CHAIN_ID, &key.to_bytes()[..]);

    let (_api, client) = ServiceBuilder::default()
        .role(ServiceRole::Api)
        .relayer(relayer.config.clone())
        .build(&anvil, &db_url)
        .await?;
    let v2_client = Configuration {
        bearer_access_token: Some(relayer.api_key.clone()),
        ..client.clone()
    };

    let v1_response = send_v1_tx(&client, &relayer.api_key).await;
    let Err(Error::ResponseError(v1_response)) = v1_response else {
        eyre::bail!("Expected an error response, got {v1_response:?}");
    };
    let v2_response = send_v2_tx(&v2_client).await;
    let Err(Error::ResponseError(v2_response)) = v2_response else {
        eyre::bail!("Expected an error response, got {v2_response:?}");
    };
    for (status, content) in [
        (v1_response.status, v1_response.content),
        (v2_response.status, v2_response.content),
    ] {
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{content}");
        let body: ErrorResponseBody = serde_json::from_str(&content)?;
        assert_eq!(body.error_id, "relayer_not_ready");
        assert_eq!(body.error_message, "Relayer not ready: nonce not synced");
    }
    let stored = connect_db(&db_url)
        .await?
        .get_relayer(&relayer.config.id)
        .await?
        .ok_or_else(|| eyre::eyre!("Missing relayer"))?;
    assert_eq!(stored.nonce, 0, "A rejected send took a nonce");

    let (_worker, _) = ServiceBuilder::default()
        .role(ServiceRole::Worker)
        .relayer(relayer.config.clone())
        .build(&anvil, &db_url)
        .await?;

    let SendTxResponse { tx_id } = send_v2_tx(&v2_client).await?;
    let tx = tx_sitter_client::apis::relayer_v2_api::get_transaction_v2(
        &v2_client,
        GetTransactionV2Params {
            tx_id: tx_id.clone(),
        },
    )
    .await?;
    assert_eq!(u64::try_from(tx.nonce)?, PRIOR_TXS);

    await_mined_tx_hash(&client, &relayer.api_key, &tx_id).await?;
    let provider = setup_provider(anvil.endpoint()).await?;
    let chain_nonce = provider.get_transaction_count(key.address()).await?;
    assert_eq!(chain_nonce, PRIOR_TXS + 1);

    Ok(())
}

/// Sends `count` transactions from `key` directly to `anvil`, and waits until
/// they are mined.
async fn send_txs_outside_tx_sitter(
    anvil: &AnvilInstance,
    key: PrivateKeySigner,
    count: u64,
) -> eyre::Result<()> {
    let provider = ProviderBuilder::new()
        .wallet(key)
        .connect_http(anvil.endpoint_url());

    for _ in 0..count {
        provider
            .send_transaction(
                TransactionRequest::default()
                    .with_to(ARBITRARY_ADDRESS)
                    .with_value(U256::from(1)),
            )
            .await?
            .get_receipt()
            .await?;
    }

    Ok(())
}

async fn send_v1_tx(
    client: &Configuration,
    api_key: &str,
) -> Result<
    SendTxResponse,
    Error<tx_sitter_client::apis::relayer_v1_api::CreateTransactionError>,
> {
    tx_sitter_client::apis::relayer_v1_api::create_transaction(
        client,
        CreateTransactionParams {
            api_token: api_key.to_string(),
            send_tx_request: SendTxRequest {
                to: ARBITRARY_ADDRESS.into(),
                value: U256::from(1).into(),
                gas_limit: U256::from(21_000).into(),
                ..Default::default()
            },
        },
    )
    .await
}

async fn send_v2_tx(
    client: &Configuration,
) -> Result<
    SendTxResponse,
    Error<tx_sitter_client::apis::relayer_v2_api::CreateTransactionV2Error>,
> {
    tx_sitter_client::apis::relayer_v2_api::create_transaction_v2(
        client,
        CreateTransactionV2Params {
            send_tx_request_v2: SendTxRequestV2::new(
                ARBITRARY_ADDRESS.into(),
                U256::from(1).into(),
                U256::from(21_000).into(),
            ),
        },
    )
    .await
}
