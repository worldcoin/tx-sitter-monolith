mod common;

use tx_sitter::client::ClientError;
use tx_sitter::server::ApiError;
use tx_sitter::types::{RelayerUpdate, TransactionPriority};

use crate::common::prelude::*;

const MAX_QUEUED_TXS: usize = 20;

#[tokio::test]
async fn send_too_many_txs() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (_service, client) =
        ServiceBuilder::default().build(&anvil, &db_url).await?;

    let CreateApiKeyResponse { api_key: _api_key } =
        client.create_relayer_api_key(DEFAULT_RELAYER_ID).await?;

    let CreateRelayerResponse {
        relayer_id: secondary_relayer_id,
        address: _secondary_relayer_address,
    } = client
        .create_relayer(&CreateRelayerRequest {
            name: "Secondary Relayer".to_string(),
            chain_id: DEFAULT_ANVIL_CHAIN_ID,
        })
        .await?;

    let CreateApiKeyResponse {
        api_key: secondary_api_key,
    } = client.create_relayer_api_key(&secondary_relayer_id).await?;

    // Set max queued txs
    client
        .update_relayer(
            &secondary_relayer_id,
            RelayerUpdate::default().with_max_queued_txs(MAX_QUEUED_TXS as u64),
        )
        .await?;

    let provider = setup_provider(anvil.endpoint()).await?;

    // Send a transaction
    let value: U256 = parse_units("0.01", "ether")?.into();

    for _ in 0..=MAX_QUEUED_TXS {
        client
            .send_tx(
                &secondary_api_key,
                &SendTxRequest {
                    to: ARBITRARY_ADDRESS,
                    value,
                    data: None,
                    gas_limit: U256::from(21_000),
                    priority: TransactionPriority::Regular,
                    tx_id: None,
                },
            )
            .await?;
    }

    // Sending one more tx should fail
    let result = client
        .send_tx(
            &secondary_api_key,
            &SendTxRequest {
                to: ARBITRARY_ADDRESS,
                value,
                data: None,
                gas_limit: U256::from(21_000),
                priority: TransactionPriority::Regular,
                tx_id: None,
            },
        )
        .await;

    assert!(
        matches!(
            result,
            Err(ClientError::TxSitter(ApiError::TooManyTransactions { .. }))
        ),
        "Result {:?} should be too many transactions",
        result
    );

    Ok(())
}
