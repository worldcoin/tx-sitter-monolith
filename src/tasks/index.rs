use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{Block, BlockNumber, H256, U256};
use eyre::ContextCompat;
use futures::stream::FuturesUnordered;
use futures::StreamExt;

use crate::app::App;
use crate::broadcast_utils::gas_estimation::{
    estimate_percentile_fees, FeesEstimate,
};
use crate::db::data::NextBlock;
use crate::db::BlockTxStatus;

const BLOCK_FEE_HISTORY_SIZE: usize = 10;
const TRAILING_BLOCK_OFFSET: u64 = 5;
const FEE_PERCENTILES: [f64; 5] = [5.0, 25.0, 50.0, 75.0, 95.0];

pub async fn index_blocks(app: Arc<App>) -> eyre::Result<()> {
    loop {
        let next_block_numbers = app.db.get_next_block_numbers().await?;

        for next_block in next_block_numbers {
            update_block(app.clone(), next_block).await?;
        }

        let (update_mined, update_finalized) = tokio::join!(
            app.db.update_transactions(BlockTxStatus::Mined),
            app.db.update_transactions(BlockTxStatus::Finalized)
        );

        update_mined?;
        update_finalized?;

        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn update_block(
    app: Arc<App>,
    next_block: NextBlock,
) -> eyre::Result<()> {
    let chain_id = U256::from(next_block.chain_id);
    let rpc = app
        .rpcs
        .get(&chain_id)
        .context("Missing RPC for chain id")?;

    let block =
        fetch_block_with_fee_estimates(rpc, next_block.next_block_number)
            .await?;

    let Some((block, fee_estimates)) = block else {
        return Ok(());
    };

    let block_timestamp_seconds = block.timestamp.as_u64();
    let block_timestamp =
        DateTime::<Utc>::from_timestamp(block_timestamp_seconds as i64, 0)
            .context("Invalid timestamp")?;

    app.db
        .save_block(
            next_block.next_block_number,
            chain_id.as_u64(),
            block_timestamp,
            &block.transactions,
            Some(&fee_estimates),
            BlockTxStatus::Mined,
        )
        .await?;

    let relayer_addresses =
        app.db.fetch_relayer_addresses(chain_id.as_u64()).await?;

    update_relayer_nonces(relayer_addresses, &app, rpc, chain_id).await?;

    if next_block.next_block_number > TRAILING_BLOCK_OFFSET {
        let block = fetch_block(
            rpc,
            next_block.next_block_number - TRAILING_BLOCK_OFFSET,
        )
        .await?
        .context("Missing trailing block")?;

        let block_timestamp_seconds = block.timestamp.as_u64();
        let block_timestamp =
            DateTime::<Utc>::from_timestamp(block_timestamp_seconds as i64, 0)
                .context("Invalid timestamp")?;

        app.db
            .save_block(
                next_block.next_block_number,
                chain_id.as_u64(),
                block_timestamp,
                &block.transactions,
                None,
                BlockTxStatus::Finalized,
            )
            .await?;
    }

    Ok(())
}

async fn update_relayer_nonces(
    relayer_addresses: Vec<ethers::types::H160>,
    app: &Arc<App>,
    rpc: &Provider<Http>,
    chain_id: U256,
) -> Result<(), eyre::Error> {
    let mut futures = FuturesUnordered::new();

    for relayer_address in relayer_addresses {
        let app = app.clone();

        futures.push(async move {
            let tx_count =
                rpc.get_transaction_count(relayer_address, None).await?;

            app.db
                .update_relayer_nonce(
                    chain_id.as_u64(),
                    relayer_address,
                    tx_count.as_u64(),
                )
                .await?;

            Result::<(), eyre::Report>::Ok(())
        })
    }

    while let Some(result) = futures.next().await {
        result?;
    }

    Ok(())
}

pub async fn fetch_block_with_fee_estimates(
    rpc: &Provider<Http>,
    block_id: impl Into<BlockNumber>,
) -> eyre::Result<Option<(Block<H256>, FeesEstimate)>> {
    let block_id = block_id.into();

    let block = rpc.get_block(block_id).await?;

    let Some(block) = block else {
        return Ok(None);
    };

    let fee_history = rpc
        .fee_history(BLOCK_FEE_HISTORY_SIZE, block_id, &FEE_PERCENTILES)
        .await?;

    let fee_estimates = estimate_percentile_fees(&fee_history)?;

    Ok(Some((block, fee_estimates)))
}

pub async fn fetch_block(
    rpc: &Provider<Http>,
    block_id: impl Into<BlockNumber>,
) -> eyre::Result<Option<Block<H256>>> {
    let block_id = block_id.into();

    let block = rpc.get_block(block_id).await?;

    let Some(block) = block else {
        return Ok(None);
    };

    Ok(Some(block))
}
