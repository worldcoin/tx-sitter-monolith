use std::sync::Arc;

use chrono::{DateTime, Utc};
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::BlockNumber;
use eyre::{Context, ContextCompat};
use futures::stream::FuturesUnordered;
use futures::StreamExt;

use crate::app::App;
use crate::broadcast_utils::gas_estimation::{
    estimate_percentile_fees, FeesEstimate,
};

const BLOCK_FEE_HISTORY_SIZE: usize = 10;
const FEE_PERCENTILES: [f64; 5] = [5.0, 25.0, 50.0, 75.0, 95.0];

pub async fn index_chain(app: Arc<App>, chain_id: u64) -> eyre::Result<()> {
    loop {
        let ws_rpc = app.fetch_ws_provider(chain_id).await?;
        let rpc = app.fetch_http_provider(chain_id).await?;

        let mut blocks_stream = ws_rpc.subscribe_blocks().await?;

        while let Some(block) = blocks_stream.next().await {
            let block_number =
                block.number.context("Missing block number")?.as_u64();

            tracing::info!(block_number, "Indexing block");

            let block_timestamp_seconds = block.timestamp.as_u64();
            let block_timestamp = DateTime::<Utc>::from_timestamp(
                block_timestamp_seconds as i64,
                0,
            )
            .context("Invalid timestamp")?;

            // TODO: We don't need to do this for every block for a given chain
            //       Add a separate task to do this periodically for the latest block
            let fee_estimates = fetch_block_fee_estimates(&rpc, block_number)
                .await
                .context("Failed to fetch fee estimates")?;

            app.db
                .save_block(
                    block.number.unwrap().as_u64(),
                    chain_id,
                    block_timestamp,
                    &block.transactions,
                    Some(&fee_estimates),
                )
                .await?;

            // TODO: This should be a per network setting
            let finalization_timestamp =
                Utc::now() - chrono::Duration::seconds(60 * 60 * 24 * 7);

            app.db
                .update_transactions(chain_id, finalization_timestamp)
                .await?;
        }
        let relayer_addresses =
            app.db.fetch_relayer_addresses(chain_id).await?;

        update_relayer_nonces(relayer_addresses, &app, &rpc, chain_id).await?;
    }
}

async fn update_relayer_nonces(
    relayer_addresses: Vec<ethers::types::H160>,
    app: &Arc<App>,
    rpc: &Provider<Http>,
    chain_id: u64,
) -> Result<(), eyre::Error> {
    let mut futures = FuturesUnordered::new();

    for relayer_address in relayer_addresses {
        let app = app.clone();

        futures.push(async move {
            let tx_count =
                rpc.get_transaction_count(relayer_address, None).await?;

            app.db
                .update_relayer_nonce(
                    chain_id,
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

pub async fn fetch_block_fee_estimates(
    rpc: &Provider<Http>,
    block_id: impl Into<BlockNumber>,
) -> eyre::Result<FeesEstimate> {
    let block_id = block_id.into();

    let fee_history = rpc
        .fee_history(BLOCK_FEE_HISTORY_SIZE, block_id, &FEE_PERCENTILES)
        .await?;

    let fee_estimates = estimate_percentile_fees(&fee_history)?;

    Ok(fee_estimates)
}
