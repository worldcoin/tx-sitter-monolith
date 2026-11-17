use std::sync::Arc;
use std::time::Duration;

use alloy::eips::BlockNumberOrTag;
use alloy::primitives::U256;
use alloy::providers::{Provider, RootProvider};
use alloy::rpc::types::Header;
use chrono::{DateTime, Utc};
use eyre::{Context, ContextCompat};
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use tokio::time::timeout;

use crate::app::App;
use crate::broadcast_utils::gas_estimation::{
    estimate_percentile_fees, FeesEstimate,
};
use crate::db::data::RelayerInfo;

const BLOCK_FEE_HISTORY_SIZE: u64 = 10;
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
    let mut blocks_stream = ws_rpc.subscribe_blocks().await?.into_stream();

    tracing::info!("Backfilling blocks");
    if let Some(latest_block) = blocks_stream.next().await {
        backfill_to_block(app.clone(), chain_id, &rpc, latest_block).await?;
    }

    loop {
        let next_block = timeout(
            app.config.service.block_stream_timeout,
            blocks_stream.next(),
        )
        .await;

        match next_block {
            Ok(Some(block)) => {
                backfill_to_block(app.clone(), chain_id, &rpc, block).await?;
            }
            Ok(None) => {
                // Stream ended, break out of the loop
                tracing::info!("Block stream ended");
                break;
            }
            Err(_) => {
                // Timeout occurred
                tracing::warn!("Timed out waiting for a block");
                break;
            }
        }
    }

    Ok(())
}

#[tracing::instrument(skip(app, rpc, header))]
pub async fn index_block(
    app: Arc<App>,
    chain_id: u64,
    rpc: &RootProvider,
    header: Header,
) -> eyre::Result<()> {
    let block_number = header.number;

    tracing::info!(chain_id, block_number, "Indexing block");

    let block_timestamp =
        DateTime::<Utc>::from_timestamp(header.timestamp as i64, 0)
            .context("Invalid timestamp")?;

    let block = rpc
        .get_block_by_number(block_number.into())
        .await?
        .context("Missing block")?;

    let tx_hashes: Vec<_> = block.transactions.hashes().collect();

    app.db
        .save_block(block.header.number, chain_id, block_timestamp, &tx_hashes)
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

/// Indexes `latest_block`, first indexing every block between the latest one
/// saved in the database and it.
///
/// Every header from the block subscription goes through here, because the
/// subscription skips headers when indexing falls behind it, and a reconnect
/// misses the headers produced in the meantime.
#[tracing::instrument(skip(app, rpc, latest_block), level = "info")]
pub async fn backfill_to_block(
    app: Arc<App>,
    chain_id: u64,
    rpc: &RootProvider,
    latest_block: Header,
) -> eyre::Result<()> {
    let latest_block_number = latest_block.number;

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
        latest_block_number.saturating_sub(MAX_RECENT_BLOCKS_TO_CHECK)
    };

    if latest_block_number > next_block_number {
        tracing::info!(
            latest_block_number,
            next_block_number,
            "Backfilling to block"
        );

        // Backfill blocks between the last synced block and the chain head, non inclusive
        for block_number in next_block_number..latest_block_number {
            let block = rpc
                .get_block_by_number(block_number.into())
                .await?
                .context(format!(
                    "Could not get block at height {block_number}"
                ))?;

            index_block(app.clone(), chain_id, rpc, block.header).await?;
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

        let gas_price = U256::from(rpc.get_gas_price().await?);

        app.db
            .save_block_fees(
                latest_block_number,
                chain_id,
                &fee_estimates,
                gas_price,
            )
            .await?;

        let labels = [("chain_id", chain_id.to_string())];
        metrics::gauge!("gas_price", &labels).set(
            gas_price.saturating_to::<u64>() as f64
                * GAS_PRICE_FOR_METRICS_FACTOR,
        );
        metrics::gauge!("base_fee_per_gas", &labels).set(
            fee_estimates.base_fee_per_gas.saturating_to::<u64>() as f64
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
            .set(
                percentile_fee.saturating_to::<u64>() as f64
                    * GAS_PRICE_FOR_METRICS_FACTOR,
            );
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
    rpc: &RootProvider,
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
    rpc: &RootProvider,
    relayer: &RelayerInfo,
    chain_id: u64,
) -> eyre::Result<()> {
    let tx_count = rpc.get_transaction_count(relayer.address.0).await?;

    if tx_count == relayer.current_nonce {
        return Ok(());
    }

    tracing::info!(
        relayer_id = relayer.id,
        current_nonce = %relayer.current_nonce,
        nonce = %relayer.nonce,
        new_current_nonce = %tx_count,
        relayer_address = %relayer.address,
        "Updating relayer nonce"
    );

    app.db
        .update_relayer_nonce(chain_id, relayer.address.0, tx_count)
        .await?;

    Ok(())
}

pub async fn get_block_fee_estimates(
    rpc: &RootProvider,
    block_id: impl Into<BlockNumberOrTag>,
) -> eyre::Result<FeesEstimate> {
    let block_id = block_id.into();

    let fee_history = rpc
        .get_fee_history(BLOCK_FEE_HISTORY_SIZE, block_id, &FEE_PERCENTILES)
        .await?;

    let fee_estimates = estimate_percentile_fees(&fee_history)?;

    Ok(fee_estimates)
}
