mod common;

use alloy::providers::ext::AnvilApi;
use sqlx::{Connection, PgConnection};
use tx_sitter_client::apis::relayer_v1_api::CreateTransactionParams;

use crate::common::prelude::*;

/// More blocks than the block subscription buffers for a subscriber that
/// falls behind.
const BURST_BLOCKS: u64 = 50;

#[tokio::test]
async fn index_skipped_blocks() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;

    let (_service, client) =
        ServiceBuilder::default().build(&anvil, &db_url).await?;
    let api_key = DEFAULT_API_KEY.to_string();

    let provider = setup_provider(anvil.endpoint()).await?;

    // Saving a block waits for this lock, so indexing stalls while the block
    // subscription keeps receiving headers
    let mut blocks_lock = PgConnection::connect(&db_url).await?;
    sqlx::query("BEGIN").execute(&mut blocks_lock).await?;
    sqlx::query("LOCK TABLE blocks IN EXCLUSIVE MODE")
        .execute(&mut blocks_lock)
        .await?;

    let value = U256::from(1_000);
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
    .await?;
    await_balance(&provider, value, ARBITRARY_ADDRESS).await?;

    provider.anvil_mine(Some(BURST_BLOCKS), None).await?;
    let chain_head = provider.get_block_number().await?;

    // Let the burst's headers reach tx-sitter's subscription before indexing
    // resumes
    tokio::time::sleep(Duration::from_secs(2)).await;
    blocks_lock.close().await?;

    await_mined_tx_hash(&client, &api_key, &tx.tx_id).await?;

    let mut db = PgConnection::connect(&db_url).await?;
    let block_numbers = await_indexed_blocks(&mut db, chain_head).await?;
    let missing_blocks: Vec<_> = block_numbers
        .windows(2)
        .filter(|pair| pair[1] != pair[0] + 1)
        .map(|pair| pair[0] + 1..pair[1])
        .collect();

    assert!(
        missing_blocks.is_empty(),
        "Blocks missing from the index: {missing_blocks:?}"
    );

    Ok(())
}

/// Waits until `block_number` is indexed and returns every indexed block
/// number up to it, in ascending order.
async fn await_indexed_blocks(
    db: &mut PgConnection,
    block_number: u64,
) -> eyre::Result<Vec<u64>> {
    for _ in 0..30 {
        let block_numbers: Vec<i64> = sqlx::query_scalar(
            "SELECT block_number FROM blocks WHERE chain_id = $1 AND block_number <= $2 ORDER BY block_number",
        )
        .bind(DEFAULT_ANVIL_CHAIN_ID as i64)
        .bind(block_number as i64)
        .fetch_all(&mut *db)
        .await?;

        if block_numbers.last() == Some(&(block_number as i64)) {
            return Ok(block_numbers.into_iter().map(|n| n as u64).collect());
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    eyre::bail!("Block {block_number} was not indexed in time");
}
