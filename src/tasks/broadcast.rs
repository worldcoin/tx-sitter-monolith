use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use alloy::consensus::TxEip1559;
use alloy::primitives::{keccak256, TxKind};
use eyre::{Context, ContextCompat};
use futures::FutureExt;
use itertools::Itertools;
use tokio::task::{JoinError, JoinSet};
use tokio::time::timeout;

use crate::app::App;
use crate::broadcast_utils::{
    calculate_gas_fees_from_estimates, send_raw_tx,
    should_send_relayer_transactions, SendOutcome, MAX_SEND_ATTEMPTS,
};
use crate::db::data::RelayerInfo;
use crate::db::UnsentTx;
use crate::task_runner::TaskContext;

const BROADCAST_INTERVAL: Duration = Duration::from_secs(2);

/// How much longer than its send attempts the broadcast of a transaction may
/// take, for signing, the database and the backoff between attempts
const TX_BROADCAST_MARGIN: Duration = Duration::from_secs(30);

/// Broadcasts the unsent transactions of every relayer, in nonce order.
///
/// Each relayer's transactions are broadcast by a task of their own. A relayer
/// whose previous broadcast is still running is skipped rather than waited
/// for, so a slow relayer or chain doesn't delay the others, and counted in
/// the `relayer_broadcast_skipped_busy` metric. A transaction whose broadcast
/// outlives its deadline ends its relayer's broadcast, see
/// [`broadcast_relayer_txs`]. Once shutdown is requested, this waits for the
/// running broadcasts, which stop after their current transaction.
pub async fn broadcast_txs(
    app: Arc<App>,
    ctx: TaskContext,
) -> eyre::Result<()> {
    let mut broadcasts = JoinSet::new();
    let mut busy_relayers = HashSet::new();
    let tx_deadline = app.config.service.rpc_timeout * MAX_SEND_ATTEMPTS
        + TX_BROADCAST_MARGIN;

    while !ctx.is_shutting_down() {
        while let Some(Some(finished)) = broadcasts.join_next().now_or_never() {
            finish_broadcast(finished, &mut busy_relayers)?;
        }

        let txs = app.db.get_unsent_txs().await?;

        for (relayer_id, txs) in sort_txs_by_relayer(txs) {
            if !busy_relayers.insert(relayer_id.clone()) {
                metrics::counter!(
                    "relayer_broadcast_skipped_busy",
                    &[("relayer_id", relayer_id)]
                )
                .increment(1);
                continue;
            }

            let (app, ctx) = (app.clone(), ctx.clone());
            broadcasts.spawn(async move {
                let result = broadcast_relayer_txs(
                    &app,
                    &ctx,
                    &relayer_id,
                    txs,
                    tx_deadline,
                )
                .await;

                (relayer_id, result)
            });
        }

        ctx.report_progress();
        ctx.idle(BROADCAST_INTERVAL).await;
    }

    while let Some(finished) = broadcasts.join_next().await {
        finish_broadcast(finished, &mut busy_relayers)?;
    }

    Ok(())
}

fn finish_broadcast(
    finished: Result<(String, eyre::Result<()>), JoinError>,
    busy_relayers: &mut HashSet<String>,
) -> eyre::Result<()> {
    let (relayer_id, result) =
        finished.context("Relayer broadcast panicked")?;
    busy_relayers.remove(&relayer_id);

    if let Err(err) = result {
        tracing::error!(
            relayer_id,
            error = ?err,
            "Failed broadcasting transactions"
        );
    }

    Ok(())
}

/// Broadcasts `txs` of `relayer_id` in order, until the node is unavailable.
///
/// A transaction whose broadcast takes longer than `tx_deadline`, such as one
/// waiting for a database lock, is given up on with an error log and the
/// `tx_broadcast_deadline_exceeded` metric, and the rest wait for the next
/// pass. Giving up is safe at any point: a transaction stored before the
/// deadline is sent again by escalation, and one that wasn't is still
/// unsent.
#[tracing::instrument(skip(app, ctx, txs))]
async fn broadcast_relayer_txs(
    app: &App,
    ctx: &TaskContext,
    relayer_id: &str,
    txs: Vec<UnsentTx>,
    tx_deadline: Duration,
) -> eyre::Result<()> {
    let relayer = app
        .db
        .get_relayer(relayer_id)
        .await?
        .context("Missing relayer")?;

    if !should_send_relayer_transactions(app, &relayer).await? {
        return Ok(());
    }

    tracing::info!(
        relayer_id,
        relayer_address = %relayer.address,
        num_txs = txs.len(),
        "Broadcasting relayer transactions"
    );

    for tx in txs {
        if ctx.is_shutting_down() {
            break;
        }

        let tx_id = tx.id.clone();
        let Ok(outcome) =
            timeout(tx_deadline, broadcast_relayer_tx(app, ctx, &relayer, tx))
                .await
        else {
            tracing::error!(
                relayer_id,
                chain_id = relayer.chain_id,
                tx_id,
                ?tx_deadline,
                "Broadcasting transaction took longer than its deadline"
            );
            metrics::counter!(
                "tx_broadcast_deadline_exceeded",
                &[("chain_id", relayer.chain_id.to_string())]
            )
            .increment(1);
            break;
        };
        let outcome = outcome?;
        ctx.report_progress();

        // The node is unavailable, so leave the rest unsigned for the next pass
        if let SendOutcome::Failed(failure) = outcome {
            if failure.is_retryable() {
                break;
            }
        }
    }

    Ok(())
}

/// Signs `tx`, stores it and only then sends it, so that the stored bytes can
/// be sent again if this send fails or the process stops before it.
#[tracing::instrument(
    skip(app, ctx, relayer, tx),
    fields(
        relayer_id = tx.relayer_id,
        relayer_address = %relayer.address,
        tx_id = tx.id
    )
)]
async fn broadcast_relayer_tx(
    app: &App,
    ctx: &TaskContext,
    relayer: &RelayerInfo,
    tx: UnsentTx,
) -> eyre::Result<SendOutcome> {
    tracing::info!(
        relayer_id = relayer.id,
        relayer_address = %relayer.address,
        tx_id = tx.id,
        nonce = tx.nonce,
        "Sending transaction"
    );

    let provider = app.http_provider(tx.chain_id).await?;
    let signer = app.load_signer(tx.key_id.clone()).await?;

    let fees = app
        .db
        .get_latest_block_fees_by_chain_id(tx.chain_id)
        .await?
        .context("Missing block fees")?;

    let max_base_fee_per_gas = fees.fee_estimates.base_fee_per_gas;

    let (max_fee_per_gas, max_priority_fee_per_gas) =
        calculate_gas_fees_from_estimates(
            &fees.fee_estimates,
            tx.priority.to_percentile_index(),
            max_base_fee_per_gas,
        );

    let eip1559_tx = TxEip1559 {
        chain_id: tx.chain_id,
        nonce: tx.nonce,
        gas_limit: tx.gas_limit.0.try_into().context("Gas limit too high")?,
        max_fee_per_gas: max_fee_per_gas
            .try_into()
            .context("Max fee per gas too high")?,
        max_priority_fee_per_gas: max_priority_fee_per_gas
            .try_into()
            .context("Max priority fee per gas too high")?,
        to: TxKind::Call(tx.tx_to.0),
        value: tx.value.0,
        access_list: Default::default(),
        input: tx.data.into(),
    };

    // Get the raw signed tx and derive the tx hash
    let raw_signed_tx = signer.raw_signed_tx(eip1559_tx).await?;
    let tx_hash = keccak256(&raw_signed_tx);

    tracing::debug!(
        relayer_id = relayer.id,
        relayer_address = %relayer.address,
        tx_id = tx.id,
        "Saving transaction"
    );
    app.db
        .insert_tx_broadcast(
            &tx.id,
            tx_hash,
            &raw_signed_tx,
            max_fee_per_gas,
            max_priority_fee_per_gas,
        )
        .await?;

    tracing::debug!(
        relayer_id = relayer.id,
        relayer_address = %relayer.address,
        tx_id = tx.id,
        "Sending transaction"
    );

    let outcome =
        send_raw_tx(&provider, ctx, relayer, &tx.id, &raw_signed_tx).await;

    if let SendOutcome::Accepted | SendOutcome::AlreadyKnown = outcome {
        tracing::info!(
            relayer_id = relayer.id,
            relayer_address = %relayer.address,
            tx_id = tx.id,
            tx_nonce = tx.nonce,
            tx_hash = ?tx_hash,
            "Transaction broadcast"
        );
    }

    Ok(outcome)
}

fn sort_txs_by_relayer(
    mut txs: Vec<UnsentTx>,
) -> HashMap<String, Vec<UnsentTx>> {
    txs.sort_unstable_by_key(|tx| tx.relayer_id.clone());
    let txs_by_relayer = txs.into_iter().group_by(|tx| tx.relayer_id.clone());

    txs_by_relayer
        .into_iter()
        .map(|(relayer_id, txs)| {
            let mut txs = txs.collect_vec();

            txs.sort_unstable_by_key(|tx| tx.nonce);

            (relayer_id, txs)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use alloy::node_bindings::Anvil;
    use alloy::primitives::{Address, U256};
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::broadcast_utils::gas_estimation::FeesEstimate;
    use crate::test_utils::{setup_app, CHAIN_ID};
    use crate::types::TransactionPriority;

    #[tokio::test]
    async fn gives_up_on_a_tx_waiting_for_a_lock_at_its_deadline(
    ) -> eyre::Result<()> {
        let anvil = Anvil::new()
            .chain_id(CHAIN_ID)
            .arg("--no-mining")
            .try_spawn()?;
        let (app, _db_container) =
            setup_app(&anvil.endpoint(), &anvil.ws_endpoint()).await?;
        let relayer_id = uuid::Uuid::new_v4().to_string();
        app.db
            .create_relayer(
                &relayer_id,
                "relayer",
                CHAIN_ID,
                &hex::encode(anvil.keys()[0].to_bytes()),
                anvil.addresses()[0],
            )
            .await?;
        let gwei = U256::from(1_000_000_000u64);
        app.db
            .save_block(1, CHAIN_ID, chrono::Utc::now(), &[])
            .await?;
        app.db
            .save_block_fees(
                1,
                CHAIN_ID,
                &FeesEstimate {
                    base_fee_per_gas: gwei,
                    percentile_fees: vec![gwei; 5],
                },
                gwei,
                None,
            )
            .await?;
        app.db
            .create_transaction(
                "tx_id",
                Address::with_last_byte(1),
                &[],
                U256::from(1),
                U256::from(21_000),
                TransactionPriority::Regular,
                None,
                &relayer_id,
                None,
                None,
            )
            .await?;
        let ctx = TaskContext::new(CancellationToken::new());

        // Storing the broadcast waits for a reset that holds the relayer
        let mut reset = app.db.pool.begin().await?;
        sqlx::query("SELECT 1 FROM relayers WHERE id = $1 FOR UPDATE")
            .bind(&relayer_id)
            .execute(reset.as_mut())
            .await?;

        let tx_deadline = Duration::from_secs(1);
        let started = Instant::now();
        let txs = app.db.get_unsent_txs().await?;
        broadcast_relayer_txs(&app, &ctx, &relayer_id, txs, tx_deadline)
            .await?;
        assert!(
            started.elapsed() < tx_deadline * 2,
            "{:?}",
            started.elapsed()
        );

        reset.rollback().await?;
        let txs = app.db.get_unsent_txs().await?;
        assert_eq!(txs.len(), 1);
        broadcast_relayer_txs(&app, &ctx, &relayer_id, txs, tx_deadline * 10)
            .await?;
        assert!(app.db.get_unsent_txs().await?.is_empty());

        Ok(())
    }
}
