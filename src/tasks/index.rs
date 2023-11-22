use std::sync::Arc;
use std::time::Duration;

use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{Block, BlockNumber, H256, U256};
use eyre::ContextCompat;

use crate::app::App;
use crate::broadcast_utils::gas_estimation::{
    estimate_percentile_fees, FeesEstimate,
};
use crate::db::BlockTxStatus;

const BLOCK_FEE_HISTORY_SIZE: usize = 10;
const TRAILING_BLOCK_OFFSET: u64 = 5;
const FEE_PERCENTILES: [f64; 5] = [5.0, 25.0, 50.0, 75.0, 95.0];

pub async fn index_blocks(app: Arc<App>) -> eyre::Result<()> {
    loop {
        let next_block_numbers = app.db.get_next_block_numbers().await?;

        // TODO: Parallelize
        for (block_number, chain_id) in next_block_numbers {
            let chain_id = U256::from(chain_id);
            let rpc = app
                .rpcs
                .get(&chain_id)
                .context("Missing RPC for chain id")?;

            if let Some((block, fee_estimates)) =
                fetch_block_with_fee_estimates(rpc, block_number).await?
            {
                app.db
                    .save_block(
                        block_number,
                        chain_id.as_u64(),
                        &block.transactions,
                        Some(&fee_estimates),
                        BlockTxStatus::Mined,
                    )
                    .await?;

                let relayer_addresses =
                    app.db.fetch_relayer_addresses(chain_id.as_u64()).await?;

                // TODO: Parallelize
                for relayer_address in relayer_addresses {
                    let tx_count = rpc
                        .get_transaction_count(relayer_address, None)
                        .await?;

                    app.db
                        .update_relayer_nonce(
                            chain_id.as_u64(),
                            relayer_address,
                            tx_count.as_u64(),
                        )
                        .await?;
                }

                if block_number > TRAILING_BLOCK_OFFSET {
                    let block =
                        fetch_block(rpc, block_number - TRAILING_BLOCK_OFFSET)
                            .await?
                            .context("Missing trailing block")?;

                    app.db
                        .save_block(
                            block_number,
                            chain_id.as_u64(),
                            &block.transactions,
                            None,
                            BlockTxStatus::Finalized,
                        )
                        .await?;
                }
            } else {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }

        app.db.update_transactions(BlockTxStatus::Mined).await?;
        app.db.update_transactions(BlockTxStatus::Finalized).await?;
    }
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
