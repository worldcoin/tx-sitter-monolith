use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{Block, BlockNumber, H256};
use eyre::{Context, ContextCompat};
use futures::stream::FuturesUnordered;
use futures::StreamExt;

use crate::app::App;
use crate::broadcast_utils::gas_estimation::{
    estimate_percentile_fees, FeesEstimate,
};

const BLOCK_FEE_HISTORY_SIZE: usize = 10;
const FEE_PERCENTILES: [f64; 5] = [5.0, 25.0, 50.0, 75.0, 95.0];
const TIME_BETWEEN_FEE_ESTIMATION_SECONDS: u64 = 30;

const GAS_PRICE_FOR_METRICS_FACTOR: f64 = 1e-9;

pub async fn index_chain(app: Arc<App>, chain_id: u64) -> eyre::Result<()> {
    loop {
        let ws_rpc = app.ws_provider(chain_id).await?;
        let rpc = app.http_provider(chain_id).await?;

        // Subscribe to new block with the WS client which uses an unbounded receiver, buffering the stream
        let mut blocks_stream = ws_rpc.subscribe_blocks().await?;

        // Get the latest block from the db
        let next_block_number =
            app.db.get_latest_block_number(chain_id).await? + 1;

        // Get the first block from the stream and backfill any missing blocks
        if let Some(latest_block) = blocks_stream.next().await {
            let latest_block_number = latest_block
                .number
                .context("Missing block number")?
                .as_u64();

            if latest_block_number > next_block_number {
                // Backfill blocks between the last synced block and the chain head
                for block_number in next_block_number..latest_block_number {
                    let block = rpc
                        .get_block::<BlockNumber>(block_number.into())
                        .await?
                        .context(format!(
                            "Could not get block at height {}",
                            block_number
                        ))?;

                    index_block(app.clone(), chain_id, &rpc, block).await?;
                }

                // Index the latest block after backfilling
                index_block(app.clone(), chain_id, &rpc, latest_block).await?;
            }
        }

        // Index incoming blocks from the stream
        while let Some(block) = blocks_stream.next().await {
            index_block(app.clone(), chain_id, &rpc, block).await?;
        }
    }
}

pub async fn index_block(
    app: Arc<App>,
    chain_id: u64,
    rpc: &Provider<Http>,
    block: Block<H256>,
) -> eyre::Result<()> {
    let block_number = block.number.context("Missing block number")?.as_u64();

    tracing::info!(block_number, "Indexing block");

    let block_timestamp_seconds = block.timestamp.as_u64();
    let block_timestamp =
        DateTime::<Utc>::from_timestamp(block_timestamp_seconds as i64, 0)
            .context("Invalid timestamp")?;

    let block = rpc
        .get_block(block_number)
        .await?
        .context("Missing block")?;

    app.db
        .save_block(
            block.number.unwrap().as_u64(),
            chain_id,
            block_timestamp,
            &block.transactions,
        )
        .await?;

    let mined_txs = app.db.mine_txs(chain_id).await?;

    let metric_labels: [(&str, String); 1] =
        [("chain_id", chain_id.to_string())];
    for tx in mined_txs {
        tracing::info!(
            id = tx.0,
            hash = ?tx.1,
            "Tx mined"
        );

        metrics::increment_counter!("tx_mined", &metric_labels);
    }

    let relayer_addresses = app.db.get_relayer_addresses(chain_id).await?;

    update_relayer_nonces(relayer_addresses, &app, &rpc, chain_id).await?;
    Ok(())
}

pub async fn estimate_gas(app: Arc<App>, chain_id: u64) -> eyre::Result<()> {
    let rpc = app.http_provider(chain_id).await?;

    loop {
        let latest_block_number = app
            .db
            .get_latest_block_number_without_fee_estimates(chain_id)
            .await?;

        let Some(latest_block_number) = latest_block_number else {
            tracing::info!("No blocks to estimate fees for");

            tokio::time::sleep(Duration::from_secs(2)).await;

            continue;
        };

        tracing::info!(block_number = latest_block_number, "Estimating fees");

        let fee_estimates = get_block_fee_estimates(&rpc, latest_block_number)
            .await
            .context("Failed to fetch fee estimates")?;

        let gas_price = rpc.get_gas_price().await?;

        app.db
            .save_block_fees(
                latest_block_number,
                chain_id,
                &fee_estimates,
                gas_price,
            )
            .await?;

        let labels = [("chain_id", chain_id.to_string())];
        metrics::gauge!(
            "gas_price",
            gas_price.as_u64() as f64 * GAS_PRICE_FOR_METRICS_FACTOR,
            &labels
        );
        metrics::gauge!(
            "base_fee_per_gas",
            fee_estimates.base_fee_per_gas.as_u64() as f64
                * GAS_PRICE_FOR_METRICS_FACTOR,
            &labels
        );

        for (i, percentile) in FEE_PERCENTILES.iter().enumerate() {
            let percentile_fee = fee_estimates.percentile_fees[i];

            metrics::gauge!(
                "percentile_fee",
                percentile_fee.as_u64() as f64 * GAS_PRICE_FOR_METRICS_FACTOR,
                &[
                    ("chain_id", chain_id.to_string()),
                    ("percentile", percentile.to_string()),
                ]
            );
        }

        tokio::time::sleep(Duration::from_secs(
            TIME_BETWEEN_FEE_ESTIMATION_SECONDS,
        ))
        .await;
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

            tracing::info!(
                nonce = ?tx_count,
                ?relayer_address,
                "Updating relayer nonce"
            );

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

pub async fn get_block_fee_estimates(
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
