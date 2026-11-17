use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use alloy::consensus::TxEip1559;
use alloy::primitives::{keccak256, TxKind};
use alloy::providers::Provider;
use eyre::{Context, ContextCompat};
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use itertools::Itertools;

use crate::app::App;
use crate::broadcast_utils::{
    calculate_gas_fees_from_estimates, should_send_relayer_transactions,
};
use crate::db::data::RelayerInfo;
use crate::db::UnsentTx;
use crate::task_runner::TaskContext;

const NO_TXS_SLEEP_DURATION: Duration = Duration::from_secs(2);

pub async fn broadcast_txs(
    app: Arc<App>,
    ctx: TaskContext,
) -> eyre::Result<()> {
    while !ctx.is_shutting_down() {
        let txs = app.db.get_unsent_txs().await?;
        let num_txs = txs.len();

        let txs_by_relayer = sort_txs_by_relayer(txs);

        let mut futures = FuturesUnordered::new();

        for (relayer_id, txs) in txs_by_relayer {
            futures.push(broadcast_relayer_txs(&app, &ctx, relayer_id, txs));
        }

        while let Some(result) = futures.next().await {
            if let Err(err) = result {
                tracing::error!(error = ?err, "Failed broadcasting transactions");
            }
        }

        ctx.report_progress();

        if num_txs == 0 {
            ctx.idle(NO_TXS_SLEEP_DURATION).await;
        }
    }

    Ok(())
}

#[tracing::instrument(skip(app, ctx, txs))]
async fn broadcast_relayer_txs(
    app: &App,
    ctx: &TaskContext,
    relayer_id: String,
    txs: Vec<UnsentTx>,
) -> eyre::Result<()> {
    if txs.is_empty() {
        return Ok(());
    }

    let relayer = app
        .db
        .get_relayer(&relayer_id)
        .await?
        .context("Missing relayer")?;

    if !should_send_relayer_transactions(app, &relayer).await? {
        tracing::warn!(
            relayer_id,
            relayer_address = %relayer.address,
            "Skipping relayer broadcasts"
        );

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

        broadcast_relayer_tx(app, &relayer, tx).await?;
        ctx.report_progress();
    }

    Ok(())
}

#[tracing::instrument(
    skip(app, relayer, tx),
    fields(
        relayer_id = tx.relayer_id,
        relayer_address = %relayer.address,
        tx_id = tx.id
    )
)]
async fn broadcast_relayer_tx(
    app: &App,
    relayer: &RelayerInfo,
    tx: UnsentTx,
) -> eyre::Result<()> {
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

    if let Err(err) = provider.send_raw_transaction(&raw_signed_tx).await {
        tracing::error!(
            relayer_id = relayer.id,
            relayer_address = %relayer.address,
            tx_id = tx.id,
            error = ?err,
            "Failed to send transaction"
        );
        return Ok(());
    }

    tracing::info!(
        relayer_id = relayer.id,
        relayer_address = %relayer.address,
        tx_id = tx.id,
        tx_nonce = tx.nonce,
        tx_hash = ?tx_hash,
        "Transaction broadcast"
    );

    Ok(())
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
