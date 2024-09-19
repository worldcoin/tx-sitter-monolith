mod common;

use poem::http;
use tx_sitter_client::apis::admin_v1_api::{
    CreateRelayerParams, RelayerCreateApiKeyParams, UpdateRelayerParams,
};
use tx_sitter_client::apis::relayer_v1_api::CreateTransactionParams;
use tx_sitter_client::apis::Error;

use crate::common::prelude::*;

const MAX_QUEUED_TXS: usize = 20;

#[tokio::test]
async fn send_too_many_txs() -> eyre::Result<()> {
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

    let CreateRelayerResponse {
        relayer_id: secondary_relayer_id,
        address: secondary_relayer_address,
    } = tx_sitter_client::apis::admin_v1_api::create_relayer(
        &client,
        CreateRelayerParams {
            create_relayer_request: CreateRelayerRequest {
                name: "Secondary Relayer".to_string(),
                chain_id: DEFAULT_ANVIL_CHAIN_ID as i32,
            },
        },
    )
    .await?;

    let provider = setup_provider(anvil.endpoint()).await?;
    let init_value: U256 = parse_units("1", "ether")?.into();

    // Send some funds to created relayer
    tx_sitter_client::apis::relayer_v1_api::create_transaction(
        &client,
        CreateTransactionParams {
            api_token: api_key.clone(),
            send_tx_request: SendTxRequest {
                to: secondary_relayer_address.clone(),
                value: init_value.into(),
                data: None,
                gas_limit: U256::from(21_000).into(),
                priority: Some(TransactionPriority::Regular),
                tx_id: None,
                blobs: None,
            },
        },
    )
    .await?;

    tracing::info!("Waiting for secondary relayer initial balance");
    await_balance(&provider, init_value, secondary_relayer_address.clone().0)
        .await?;

    let CreateApiKeyResponse {
        api_key: secondary_api_key,
    } = tx_sitter_client::apis::admin_v1_api::relayer_create_api_key(
        &client,
        RelayerCreateApiKeyParams {
            relayer_id: secondary_relayer_id.clone(),
        },
    )
    .await?;

    tracing::info!("Updating relayer");
    // Set max queued txs
    tx_sitter_client::apis::admin_v1_api::update_relayer(
        &client,
        UpdateRelayerParams {
            relayer_id: secondary_relayer_id.clone(),
            relayer_update_request: RelayerUpdateRequest {
                max_queued_txs: Some(MAX_QUEUED_TXS as i32),
                ..Default::default()
            },
        },
    )
    .await?;

    // Send a transaction
    let value: U256 = parse_units("0.01", "ether")?.into();

    tracing::info!("Sending txs");
    for _ in 0..=MAX_QUEUED_TXS {
        tx_sitter_client::apis::relayer_v1_api::create_transaction(
            &client,
            CreateTransactionParams {
                api_token: secondary_api_key.clone(),
                send_tx_request: SendTxRequest {
                    to: ARBITRARY_ADDRESS.into(),
                    value: value.into(),
                    data: None,
                    gas_limit: U256::from(21_000).into(),
                    priority: Some(TransactionPriority::Regular),
                    tx_id: None,
                    blobs: None,
                },
            },
        )
        .await?;
    }

    // Sending one more tx should fail
    let res = tx_sitter_client::apis::relayer_v1_api::create_transaction(
        &client,
        CreateTransactionParams {
            api_token: secondary_api_key.clone(),
            send_tx_request: SendTxRequest {
                to: ARBITRARY_ADDRESS.into(),
                value: value.into(),
                data: None,
                gas_limit: U256::from(21_000).into(),
                priority: Some(TransactionPriority::Regular),
                tx_id: None,
                blobs: None,
            },
        },
    )
    .await;

    if let Err(Error::ResponseError(e)) = res {
        assert_eq!(e.status, http::StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(e.content, "Relayer queue is full");

        return Ok(());
    }

    // Accumulate total value + gas budget
    let send_value = value * (MAX_QUEUED_TXS + 1);
    let total_required_value = send_value + parse_units("1", "ether")?;

    tx_sitter_client::apis::relayer_v1_api::create_transaction(
        &client,
        CreateTransactionParams {
            api_token: api_key.clone(),
            send_tx_request: SendTxRequest {
                to: secondary_relayer_address.clone(),
                value: total_required_value.into(),
                data: None,
                gas_limit: U256::from(21_000).into(),
                priority: Some(TransactionPriority::Regular),
                tx_id: None,
                blobs: None,
            },
        },
    )
    .await?;

    tracing::info!("Waiting for secondary relayer balance");
    await_balance(
        &provider,
        total_required_value,
        secondary_relayer_address.clone().0,
    )
    .await?;

    tracing::info!("Waiting for queued up txs to be processed");
    await_balance(&provider, send_value, ARBITRARY_ADDRESS).await?;

    Ok(())
}
