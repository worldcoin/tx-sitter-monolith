use std::collections::HashMap;
use std::sync::Arc;

use alloy::consensus::TxEip1559;
use alloy::primitives::{TxKind, B256, U256};
use alloy::providers::{Provider, RootProvider};
use eyre::{Context, ContextCompat};
use futures::stream::FuturesUnordered;
use futures::StreamExt;

use crate::app::App;
use crate::broadcast_utils::should_send_relayer_transactions;
use crate::db::data::RelayerInfo;
use crate::db::TxForEscalation;
use crate::keys::UniversalSigner;
use crate::task_runner::TaskContext;

pub async fn escalate_txs_task(
    app: Arc<App>,
    ctx: TaskContext,
) -> eyre::Result<()> {
    while !ctx.is_shutting_down() {
        escalate_txs(&app, &ctx).await?;

        ctx.idle(app.config.service.escalation_interval).await;
    }

    Ok(())
}

#[tracing::instrument(skip(app, ctx))]
async fn escalate_txs(app: &App, ctx: &TaskContext) -> eyre::Result<()> {
    tracing::info!("Escalating transactions");

    let txs_for_escalation = app
        .db
        .get_txs_for_escalation(
            app.config.service.escalation_interval,
            app.config.service.max_escalations,
        )
        .await?;

    tracing::info!("Got {} transactions to escalate", txs_for_escalation.len());

    let txs_for_escalation = split_txs_per_relayer(txs_for_escalation);

    let mut futures = FuturesUnordered::new();

    for (relayer_id, txs) in txs_for_escalation {
        futures.push(escalate_relayer_txs(app, ctx, relayer_id, txs));
    }

    while let Some(result) = futures.next().await {
        if let Err(err) = result {
            tracing::error!(error = ?err, "Failed escalating txs");
        }
    }

    Ok(())
}

#[tracing::instrument(skip(app, ctx, txs))]
async fn escalate_relayer_txs(
    app: &App,
    ctx: &TaskContext,
    relayer_id: String,
    txs: Vec<TxForEscalation>,
) -> eyre::Result<()> {
    let relayer = app
        .db
        .get_relayer(&relayer_id)
        .await?
        .context("Missing relayer")?;

    if txs.is_empty() {
        tracing::info!("No transactions to escalate");
    }

    for tx in txs {
        if ctx.is_shutting_down() {
            break;
        }

        escalate_relayer_tx(app, &relayer, tx).await?;
        ctx.report_progress();
    }

    Ok(())
}

#[tracing::instrument(
    skip(app, relayer, tx),
    fields(
        relayer_id = relayer.id,
        relayer_address = %relayer.address,
        tx_id = tx.id
    )
)]
async fn escalate_relayer_tx(
    app: &App,
    relayer: &RelayerInfo,
    tx: TxForEscalation,
) -> eyre::Result<()> {
    if !should_send_relayer_transactions(app, relayer).await? {
        tracing::warn!(
            relayer_id = relayer.id,
            relayer_address = %relayer.address,
            "Skipping relayer escalations"
        );

        return Ok(());
    }

    tracing::info!(
        relayer_id = relayer.id,
        relayer_address = %relayer.address,
        tx_id = tx.id,
        escalation_count = tx.escalation_count,
        "Escalating transaction"
    );

    let escalation = tx.escalation_count + 1;

    let provider = app.http_provider(tx.chain_id).await?;
    let signer = app.load_signer(tx.key_id.clone()).await?;

    let fees = app
        .db
        .get_latest_block_fees_by_chain_id(tx.chain_id)
        .await?
        .context("Missing block")?;

    // Min increase of 20% on the priority fee required for a replacement tx
    let factor = U256::from(100);
    let increased_gas_price_percentage =
        factor + U256::from(20 * (1 + escalation));

    let initial_max_priority_fee_per_gas =
        tx.initial_max_priority_fee_per_gas.0;

    let initial_max_fee_per_gas = tx.initial_max_fee_per_gas.0;

    let max_priority_fee_per_gas = initial_max_priority_fee_per_gas
        * increased_gas_price_percentage
        / factor;

    let max_fee_per_gas =
        max_priority_fee_per_gas + fees.fee_estimates.base_fee_per_gas;

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

    let tx_hash = match sign_and_send(&provider, &signer, eip1559_tx).await {
        Ok(tx_hash) => tx_hash,
        Err(err) => {
            tracing::error!(
                relayer_id = relayer.id,
                relayer_address = %relayer.address,
                tx_id = tx.id,
                error = ?err,
                "Failed to escalate transaction"
            );
            return Ok(());
        }
    };

    tracing::info!(
        relayer_id = relayer.id,
        relayer_address = %relayer.address,
        tx_id = tx.id,
        ?tx_hash,
        ?initial_max_priority_fee_per_gas,
        ?initial_max_fee_per_gas,
        ?max_priority_fee_per_gas,
        ?max_fee_per_gas,
        "Escalated transaction"
    );

    if tx.escalation_count + 1 >= app.config.service.max_escalations {
        tracing::warn!(
            relayer_id = relayer.id,
            relayer_address = %relayer.address,
            tx_id = tx.id,
            escalation_count = tx.escalation_count,
            "Too many escalations"
        );
    }

    app.db
        .escalate_tx(&tx.id, tx_hash, max_fee_per_gas, max_fee_per_gas)
        .await?;

    tracing::info!(
        relayer_id = relayer.id,
        relayer_address = %relayer.address,
        tx_id = tx.id,
        "Escalated transaction saved"
    );

    Ok(())
}

/// Signs `tx` and sends it, returning the hash reported by the node.
async fn sign_and_send(
    provider: &RootProvider,
    signer: &UniversalSigner,
    tx: TxEip1559,
) -> eyre::Result<B256> {
    let raw_signed_tx = signer.raw_signed_tx(tx).await?;

    let pending_tx = provider.send_raw_transaction(&raw_signed_tx).await?;

    Ok(*pending_tx.tx_hash())
}

fn split_txs_per_relayer(
    txs: Vec<TxForEscalation>,
) -> HashMap<String, Vec<TxForEscalation>> {
    let mut txs_per_relayer = HashMap::new();

    for tx in txs {
        let relayer_id = tx.relayer_id.clone();

        let txs_for_relayer =
            txs_per_relayer.entry(relayer_id).or_insert_with(Vec::new);

        txs_for_relayer.push(tx);
    }

    for (_, txs) in txs_per_relayer.iter_mut() {
        txs.sort_by_key(|tx| tx.escalation_count);
    }

    txs_per_relayer
}
