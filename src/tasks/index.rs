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
use crate::types::RelayerInfo;

const BLOCK_FEE_HISTORY_SIZE: usize = 10;
const FEE_PERCENTILES: [f64; 5] = [5.0, 25.0, 50.0, 75.0, 95.0];
const TIME_BETWEEN_FEE_ESTIMATION_SECONDS: u64 = 30;

const GAS_PRICE_FOR_METRICS_FACTOR: f64 = 1e-9;

const MAX_RECENT_BLOCKS_TO_CHECK: u64 = 60;

pub async fn index_chain(app: Arc<App>, chain_id: u64) -> eyre::Result<()> {
    loop {
        index_inner(app.clone(), chain_id).await?;
    }
}

#[tracing::instrument(skip(app), level = "info")]
async fn index_inner(app: Arc<App>, chain_id: u64) -> eyre::Result<()> {
    let ws_rpc = app.ws_provider(chain_id).await?;
    let rpc = app.http_provider(chain_id).await?;

    tracing::info!("Subscribing to new blocks");
    // Subscribe to new block with the WS client which uses an unbounded receiver, buffering the stream
    let mut blocks_stream = ws_rpc.subscribe_blocks().await?;

    // Get the first block from the stream, backfilling any missing blocks from the latest block in the db to the chain head
    tracing::info!("Backfilling blocks");
    if let Some(latest_block) = blocks_stream.next().await {
        backfill_to_block(app.clone(), chain_id, &rpc, latest_block).await?;
    }

    // Index incoming blocks from the stream
    while let Some(block) = blocks_stream.next().await {
        index_block(app.clone(), chain_id, &rpc, block).await?;
    }

    Ok(())
}

#[tracing::instrument(skip(app, rpc, block))]
pub async fn index_block(
    app: Arc<App>,
    chain_id: u64,
    rpc: &Provider<Http>,
    block: Block<H256>,
) -> eyre::Result<()> {
    let block_number = block.number.context("Missing block number")?.as_u64();

    tracing::info!(chain_id, block_number, "Indexing block");

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
            tx_id = tx.0,
            tx_hash = ?tx.1,
            "Tx mined"
        );

        metrics::counter!("tx_mined", &metric_labels).increment(1);
    }

    let relayers = app.db.get_relayers_by_chain_id(chain_id).await?;

    update_relayer_nonces(&relayers, &app, rpc, chain_id).await?;

    Ok(())
}

#[tracing::instrument(skip(app, rpc, latest_block), level = "info")]
pub async fn backfill_to_block(
    app: Arc<App>,
    chain_id: u64,
    rpc: &Provider<Http>,
    latest_block: Block<H256>,
) -> eyre::Result<()> {
    // Get the first block from the stream and backfill any missing blocks
    let latest_block_number = latest_block
        .number
        .context("Missing block number")?
        .as_u64();

    let next_block_number: u64 = if let Some(latest_db_block_number) =
        app.db.get_latest_block_number(chain_id).await?
    {
        latest_db_block_number + 1
    } else {
        tracing::info!(
            chain_id,
            "No latest block in database. Will choose best candidate."
        );

        // Because we do not store all the blocks (we clean up older blocks) there is no need
        // to scan ALL the blocks. Especially as this may take a lot of time... We are trying
        // here to move back in time "enough" to get some estimates later.
        if latest_block_number > MAX_RECENT_BLOCKS_TO_CHECK {
            latest_block_number - MAX_RECENT_BLOCKS_TO_CHECK
        } else {
            0
        }
    };

    tracing::info!(
        latest_block_number,
        next_block_number,
        "Backfilling to block"
    );

    if latest_block_number > next_block_number {
        // Backfill blocks between the last synced block and the chain head, non inclusive
        for block_number in next_block_number..latest_block_number {
            let block = rpc
                .get_block::<BlockNumber>(block_number.into())
                .await?
                .context(format!(
                    "Could not get block at height {}",
                    block_number
                ))?;

            index_block(app.clone(), chain_id, rpc, block).await?;
        }
    }

    // Index the latest block after backfilling
    index_block(app.clone(), chain_id, rpc, latest_block).await?;

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
            tracing::info!(chain_id, "No blocks to estimate fees for");

            tokio::time::sleep(Duration::from_secs(2)).await;

            continue;
        };

        tracing::info!(
            chain_id,
            block_number = latest_block_number,
            "Estimating fees"
        );

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
        metrics::gauge!("gas_price", &labels)
            .set(gas_price.as_u64() as f64 * GAS_PRICE_FOR_METRICS_FACTOR);
        metrics::gauge!("base_fee_per_gas", &labels).set(
            fee_estimates.base_fee_per_gas.as_u64() as f64
                * GAS_PRICE_FOR_METRICS_FACTOR,
        );

        for (i, percentile) in FEE_PERCENTILES.iter().enumerate() {
            let percentile_fee = fee_estimates.percentile_fees[i];

            metrics::gauge!(
                "percentile_fee",
                &[
                    ("chain_id", chain_id.to_string()),
                    ("percentile", percentile.to_string()),
                ]
            )
            .set(percentile_fee.as_u64() as f64 * GAS_PRICE_FOR_METRICS_FACTOR);
        }

        tokio::time::sleep(Duration::from_secs(
            TIME_BETWEEN_FEE_ESTIMATION_SECONDS,
        ))
        .await;
    }
}

async fn update_relayer_nonces(
    relayers: &[RelayerInfo],
    app: &App,
    rpc: &Provider<Http>,
    chain_id: u64,
) -> Result<(), eyre::Error> {
    let mut futures = FuturesUnordered::new();

    for relayer in relayers {
        futures.push(update_relayer_nonce(app, rpc, relayer, chain_id));
    }

    while let Some(result) = futures.next().await {
        result?;
    }

    Ok(())
}

#[tracing::instrument(skip(app, rpc, relayer), fields(relayer_id = relayer.id))]
async fn update_relayer_nonce(
    app: &App,
    rpc: &Provider<Http>,
    relayer: &RelayerInfo,
    chain_id: u64,
) -> eyre::Result<()> {
    let tx_count = rpc.get_transaction_count(relayer.address.0, None).await?;

    if tx_count.as_u64() == relayer.current_nonce {
        return Ok(());
    }

    tracing::info!(
        relayer_id = relayer.id,
        current_nonce = %relayer.current_nonce,
        nonce = %relayer.nonce,
        new_current_nonce = %tx_count.as_u64(),
        relayer_address = ?relayer.address.0,
        "Updating relayer nonce"
    );

    app.db
        .update_relayer_nonce(chain_id, relayer.address.0, tx_count.as_u64())
        .await?;

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
