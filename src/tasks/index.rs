use std::future::IntoFuture;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
use crate::broadcast_utils::l1_fees::{read_l1_fees, L1Fees};
use crate::db::data::{ChainNonces, RelayerNonces};
use crate::rate_limited_logs::RateLimitedLogs;
use crate::task_runner::{TaskContext, Wait};

const BLOCK_FEE_HISTORY_SIZE: u64 = 10;
const FEE_PERCENTILES: [f64; 5] = [5.0, 25.0, 50.0, 75.0, 95.0];

const GAS_PRICE_FOR_METRICS_FACTOR: f64 = 1e-9;

const MAX_RECENT_BLOCKS_TO_CHECK: u64 = 60;

/// Max amount of time the nonce sync of one chain may take at worker startup
const STARTUP_NONCE_SYNC_TIMEOUT: Duration = Duration::from_secs(30);

static REFUSED_FIRST_SYNC_LOGS: RateLimitedLogs = RateLimitedLogs::new();

/// Indexes the blocks of `chain_id` as they arrive on its block stream.
///
/// Returns an error once connecting or subscribing takes longer than
/// `rpc_timeout`, or the stream ends or goes quiet for
/// `block_stream_timeout`, so that the task runner restarts indexing after a
/// backoff. Once shutdown is requested it returns at once, also while
/// connecting or subscribing.
#[tracing::instrument(skip(app, ctx), level = "info")]
pub async fn index_chain(
    app: Arc<App>,
    ctx: TaskContext,
    chain_id: u64,
) -> eyre::Result<()> {
    let rpc_timeout = app.config.service.rpc_timeout;
    let rpc = app.http_provider(chain_id).await?;

    let ws_rpc = match ctx.wait(rpc_timeout, app.ws_provider(chain_id)).await {
        Wait::Ready(ws_rpc) => ws_rpc?,
        Wait::TimedOut => eyre::bail!("Timed out connecting to WS provider"),
        Wait::ShuttingDown => return Ok(()),
    };

    tracing::info!("Subscribing to new blocks");
    let subscription = ws_rpc.subscribe_blocks().into_future();
    let mut blocks_stream = match ctx.wait(rpc_timeout, subscription).await {
        Wait::Ready(subscription) => subscription
            .context("Subscribing to new blocks")?
            .into_stream(),
        Wait::TimedOut => eyre::bail!("Timed out subscribing to new blocks"),
        Wait::ShuttingDown => return Ok(()),
    };
    let block_stream_timeout = app.config.service.block_stream_timeout;

    while !ctx.is_shutting_down() {
        match ctx.wait(block_stream_timeout, blocks_stream.next()).await {
            Wait::Ready(Some(block)) => {
                backfill_to_block(app.clone(), &ctx, chain_id, &rpc, block)
                    .await?;
                ctx.report_progress();
            }
            Wait::Ready(None) => eyre::bail!("Block stream ended"),
            Wait::TimedOut => {
                eyre::bail!("No new block within {block_stream_timeout:?}")
            }
            Wait::ShuttingDown => break,
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

    Ok(())
}

/// Indexes `latest_block`, first indexing every block between the latest one
/// saved in the database and it, and then updates the relayers' nonces to
/// those as of `latest_block`.
///
/// Every header from the block subscription goes through here, because the
/// subscription skips headers when indexing falls behind it, and a reconnect
/// misses the headers produced in the meantime. Once shutdown is requested it
/// returns before the next block.
///
/// Reading the nonces as of the indexed block, rather than the newest one,
/// keeps a relayer's confirmed nonce from counting transactions that are
/// mined but not indexed yet.
#[tracing::instrument(skip(app, ctx, rpc, latest_block), level = "info")]
pub async fn backfill_to_block(
    app: Arc<App>,
    ctx: &TaskContext,
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
            if ctx.is_shutting_down() {
                return Ok(());
            }

            let block = rpc
                .get_block_by_number(block_number.into())
                .await?
                .context(format!(
                    "Could not get block at height {block_number}"
                ))?;

            index_block(app.clone(), chain_id, rpc, block.header).await?;
            ctx.report_progress();
        }
    }

    // Index the latest block after backfilling
    index_block(app.clone(), chain_id, rpc, latest_block).await?;

    sync_relayer_nonces(&app, rpc, chain_id, latest_block_number.into())
        .await?;

    Ok(())
}

/// Estimates the fees of `chain_id` from the latest block every
/// `service.fee_estimation_interval`, and saves them with the block.
///
/// On an OP-stack network it also reads the L1 fees and saves them with the
/// estimate. When reading them fails, it saves the L1 fees last read, so that
/// the L1 fee gate holds transactions only once those are older than
/// `op_stack.max_fee_age`.
pub async fn estimate_gas(
    app: Arc<App>,
    ctx: TaskContext,
    chain_id: u64,
) -> eyre::Result<()> {
    let reads_l1_fees = app.l1_fee_gate(chain_id).is_some();
    let mut l1_fees = if reads_l1_fees {
        app.db
            .get_latest_block_fees_by_chain_id(chain_id)
            .await?
            .and_then(|fees| fees.l1_fees)
    } else {
        None
    };

    while !ctx.is_shutting_down() {
        let latest_block_number = app
            .db
            .get_latest_block_number_without_fee_estimates(chain_id)
            .await?;

        let Some(latest_block_number) = latest_block_number else {
            tracing::info!(chain_id, "No blocks to estimate fees for");

            ctx.idle(Duration::from_secs(2)).await;

            continue;
        };

        tracing::info!(
            chain_id,
            block_number = latest_block_number,
            "Estimating fees"
        );

        let rpc = app.http_provider(chain_id).await?;
        let fee_estimates = get_block_fee_estimates(&rpc, latest_block_number)
            .await
            .context("Failed to fetch fee estimates")?;

        let gas_price = U256::from(rpc.get_gas_price().await?);

        if reads_l1_fees {
            l1_fees = refresh_l1_fees(&rpc, chain_id, l1_fees).await;
        }

        app.db
            .save_block_fees(
                latest_block_number,
                chain_id,
                &fee_estimates,
                gas_price,
                l1_fees.as_ref(),
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

        ctx.idle(app.config.service.fee_estimation_interval).await;
    }

    Ok(())
}

/// Reads the L1 fees of `chain_id`, and returns them, or `last_l1_fees` if
/// reading fails.
async fn refresh_l1_fees(
    rpc: &RootProvider,
    chain_id: u64,
    last_l1_fees: Option<L1Fees>,
) -> Option<L1Fees> {
    let labels = [("chain_id", chain_id.to_string())];

    match read_l1_fees(rpc, Utc::now()).await {
        Ok(l1_fees) => {
            metrics::gauge!("l1_base_fee", &labels).set(
                l1_fees.l1_base_fee.saturating_to::<u64>() as f64
                    * GAS_PRICE_FOR_METRICS_FACTOR,
            );
            metrics::gauge!("blob_base_fee", &labels).set(
                l1_fees.blob_base_fee.saturating_to::<u64>() as f64
                    * GAS_PRICE_FOR_METRICS_FACTOR,
            );

            Some(l1_fees)
        }
        Err(err) => {
            tracing::warn!(
                chain_id,
                error = ?err,
                last_read_at = ?last_l1_fees.map(|fees| fees.read_at),
                "Failed to read L1 fees, keeping the last ones"
            );
            metrics::counter!("l1_fee_read_failures", &labels).increment(1);

            last_l1_fees
        }
    }
}

/// Syncs the nonce of every relayer on a configured chain from its chain, as
/// indexing does with every block.
///
/// The worker runs this before it starts broadcasting, so that broadcasting
/// starts from the chains' nonces, and new relayers accept transactions
/// without waiting for each chain's next block. The API is already serving by
/// then: until a relayer's first sync it refuses the relayer's transactions,
/// see [`Database::create_transaction`].
///
/// Like indexing, it reads the nonces as of the chain's latest indexed block,
/// see [`backfill_to_block`], or as of its newest block while it has none.
/// Chains are synced concurrently, each within
/// [`STARTUP_NONCE_SYNC_TIMEOUT`]. A chain that fails is logged and left to
/// indexing, so that one unavailable chain doesn't hold up the others.
///
/// [`Database::create_transaction`]: crate::db::Database::create_transaction
pub async fn sync_relayer_nonces_at_startup(app: &App) {
    let syncs = app.config.networks.iter().map(|network| async move {
        let chain_id = network.chain_id;
        let sync = async {
            let rpc = app.http_provider(chain_id).await?;
            let block = match app.db.get_latest_block_number(chain_id).await? {
                Some(block_number) => block_number.into(),
                None => BlockNumberOrTag::Latest,
            };

            sync_relayer_nonces(app, &rpc, chain_id, block).await
        };

        let result = timeout(STARTUP_NONCE_SYNC_TIMEOUT, sync)
            .await
            .unwrap_or_else(|_| {
                Err(eyre::eyre!(
                    "Timed out after {STARTUP_NONCE_SYNC_TIMEOUT:?}"
                ))
            });
        if let Err(err) = result {
            tracing::error!(
                chain_id,
                error = ?err,
                "Syncing relayer nonces at startup failed, indexing will sync them"
            );
        }
    });

    futures::future::join_all(syncs).await;
}

/// Syncs the nonce of every relayer on `chain_id` from the chain, see
/// [`Database::update_relayer_nonce`].
///
/// [`Database::update_relayer_nonce`]: crate::db::Database::update_relayer_nonce
async fn sync_relayer_nonces(
    app: &App,
    rpc: &RootProvider,
    chain_id: u64,
    block: BlockNumberOrTag,
) -> eyre::Result<()> {
    let relayers = app.db.get_relayer_nonces(chain_id).await?;

    let mut futures = FuturesUnordered::new();

    for relayer in &relayers {
        futures.push(update_relayer_nonce(app, rpc, relayer, chain_id, block));
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
    relayer: &RelayerNonces,
    chain_id: u64,
    block: BlockNumberOrTag,
) -> eyre::Result<()> {
    let relayer_address = relayer.address.0;
    let tx_count = rpc
        .get_transaction_count(relayer_address)
        .block_id(block.into())
        .await?;

    if tx_count == relayer.current_nonce
        && tx_count <= relayer.nonce
        && !relayer.awaits_first_sync()
    {
        return Ok(());
    }

    // Only a first sync checks for transactions another signer still has in
    // flight, see `Database::update_relayer_nonce`
    let pending_tx_count = if relayer.awaits_first_sync() {
        Some(rpc.get_transaction_count(relayer_address).pending().await?)
    } else {
        None
    };

    tracing::info!(
        relayer_id = relayer.id,
        current_nonce = %relayer.current_nonce,
        nonce = %relayer.nonce,
        new_current_nonce = %tx_count,
        relayer_address = %relayer.address,
        "Updating relayer nonce"
    );

    let nonces = ChainNonces {
        latest: tx_count,
        pending: pending_tx_count,
    };
    let sync = app
        .db
        .update_relayer_nonce(chain_id, relayer_address, nonces)
        .await?;

    for (relayer_id, local_nonce) in sync.behind_chain {
        tracing::error!(
            relayer_id,
            chain_id,
            relayer_address = %relayer.address,
            local_nonce,
            chain_nonce = tx_count,
            "Relayer key sent transactions outside tx-sitter and may be in use elsewhere, moved its nonce up to the chain's"
        );
        metrics::counter!(
            "relayer_nonce_behind_chain",
            &[
                ("chain_id", chain_id.to_string()),
                ("relayer_id", relayer_id)
            ]
        )
        .increment(1);
    }

    for relayer_id in sync.first_synced {
        tracing::warn!(
            relayer_id,
            chain_id,
            relayer_address = %relayer.address,
            chain_nonce = tx_count,
            "Synced the relayer's nonce from its chain for the first time"
        );
    }

    for relayer_id in sync.pending_elsewhere {
        // Refused again on every block until the other signer's transactions
        // are mined, so log it once per period and count every refusal
        if REFUSED_FIRST_SYNC_LOGS.should_log(&relayer_id, Instant::now()) {
            tracing::error!(
                relayer_id,
                chain_id,
                relayer_address = %relayer.address,
                chain_nonce = tx_count,
                pending_nonce = pending_tx_count,
                "Relayer not synced: the key has pending transactions tx-sitter did not send"
            );
        }
        metrics::counter!(
            "relayer_first_sync_refused",
            &[
                ("chain_id", chain_id.to_string()),
                ("relayer_id", relayer_id)
            ]
        )
        .increment(1);
    }

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

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};
    use std::time::Instant;

    use futures::SinkExt;
    use postgres_docker_utils::DockerContainerGuard;
    use tokio::net::TcpListener;
    use tokio::time::timeout;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::test_utils::{
        setup_app, UnresponsiveServer, BLOCK_STREAM_TIMEOUT, CHAIN_ID,
        RPC_TIMEOUT,
    };

    #[tokio::test]
    async fn connecting_fails_within_rpc_timeout() -> eyre::Result<()> {
        let ws_rpc = UnresponsiveServer::spawn().await?;
        let (app, _db_container) = setup_app_with_ws_rpc(ws_rpc.addr).await?;

        let started = Instant::now();
        let result =
            timeout(RPC_TIMEOUT * 3, index_chain(app, ctx(), CHAIN_ID))
                .await
                .context("Connecting outlived rpc_timeout")?;
        let elapsed = started.elapsed();

        let Err(err) = result else {
            eyre::bail!("Connected without an answer");
        };
        assert_eq!(err.to_string(), "Timed out connecting to WS provider");
        assert!(elapsed >= RPC_TIMEOUT, "Took {elapsed:?}");
        assert!(elapsed <= RPC_TIMEOUT * 2, "Took {elapsed:?}");

        Ok(())
    }

    #[tokio::test]
    async fn subscribing_fails_within_rpc_timeout() -> eyre::Result<()> {
        let ws_addr = spawn_ws_rpc(WsRpc::NeverAnswers).await?;
        let (app, _db_container) = setup_app_with_ws_rpc(ws_addr).await?;

        let started = Instant::now();
        let result =
            timeout(RPC_TIMEOUT * 3, index_chain(app, ctx(), CHAIN_ID))
                .await
                .context("Subscribing outlived rpc_timeout")?;
        let elapsed = started.elapsed();

        let Err(err) = result else {
            eyre::bail!("Subscribed without an answer");
        };
        assert_eq!(err.to_string(), "Timed out subscribing to new blocks");
        assert!(elapsed >= RPC_TIMEOUT, "Took {elapsed:?}");
        assert!(elapsed <= RPC_TIMEOUT * 2, "Took {elapsed:?}");

        Ok(())
    }

    #[tokio::test]
    async fn shutdown_stops_connecting_and_subscribing() -> eyre::Result<()> {
        let unresponsive = UnresponsiveServer::spawn().await?;
        let never_answers = spawn_ws_rpc(WsRpc::NeverAnswers).await?;

        for ws_addr in [unresponsive.addr, never_answers] {
            let (app, _db_container) = setup_app_with_ws_rpc(ws_addr).await?;
            let shutdown = CancellationToken::new();
            let indexing = tokio::spawn(index_chain(
                app,
                TaskContext::new(shutdown.clone()),
                CHAIN_ID,
            ));

            tokio::time::sleep(RPC_TIMEOUT / 2).await;
            let started = Instant::now();
            shutdown.cancel();
            indexing.await??;

            let elapsed = started.elapsed();
            assert!(elapsed < RPC_TIMEOUT / 4, "Took {elapsed:?}");
        }

        Ok(())
    }

    #[tokio::test]
    async fn waiting_for_a_block_fails_after_block_stream_timeout(
    ) -> eyre::Result<()> {
        let ws_addr = spawn_ws_rpc(WsRpc::AnswersWithoutBlocks).await?;
        let (app, _db_container) = setup_app_with_ws_rpc(ws_addr).await?;

        let started = Instant::now();
        let result = timeout(
            BLOCK_STREAM_TIMEOUT * 3,
            index_chain(app, ctx(), CHAIN_ID),
        )
        .await
        .context("Waiting for a block outlived block_stream_timeout")?;
        let elapsed = started.elapsed();

        let Err(err) = result else {
            eyre::bail!("Indexing ended without a block");
        };
        assert!(
            err.to_string().starts_with("No new block within"),
            "Unexpected error: {err:?}"
        );
        assert!(elapsed >= BLOCK_STREAM_TIMEOUT, "Took {elapsed:?}");
        assert!(elapsed <= BLOCK_STREAM_TIMEOUT * 2, "Took {elapsed:?}");

        Ok(())
    }

    fn ctx() -> TaskContext {
        TaskContext::new(CancellationToken::new())
    }

    /// How a WS server from [`spawn_ws_rpc`] treats requests once it has
    /// completed the upgrade.
    #[derive(Clone, Copy)]
    enum WsRpc {
        NeverAnswers,
        /// Answers every request with a subscription id and never sends a
        /// block.
        AnswersWithoutBlocks,
    }

    async fn spawn_ws_rpc(behaviour: WsRpc) -> eyre::Result<SocketAddr> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;

        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut ws = tokio_tungstenite::accept_async(stream)
                        .await
                        .expect("WS upgrade failed");

                    while let Some(Ok(message)) = ws.next().await {
                        let Message::Text(request) = message else {
                            continue;
                        };
                        if let WsRpc::NeverAnswers = behaviour {
                            continue;
                        }

                        let request: serde_json::Value =
                            serde_json::from_str(&request)
                                .expect("Invalid JSON-RPC request");
                        let response = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": request["id"],
                            "result": "0x1",
                        });
                        ws.send(Message::text(response.to_string()))
                            .await
                            .expect("Failed to answer");
                    }
                });
            }
        });

        Ok(addr)
    }

    async fn setup_app_with_ws_rpc(
        ws_addr: SocketAddr,
    ) -> eyre::Result<(Arc<App>, DockerContainerGuard)> {
        let http_rpc = UnresponsiveServer::spawn().await?;
        let (app, db_container) = setup_app(
            &format!("http://{}", http_rpc.addr),
            &format!("ws://{ws_addr}"),
        )
        .await?;

        Ok((Arc::new(app), db_container))
    }
}
