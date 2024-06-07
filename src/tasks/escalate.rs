use std::collections::HashMap;
use std::sync::Arc;

use alloy::consensus::{SidecarBuilder, SimpleCoder};
use alloy::network::TransactionBuilder;
use alloy::providers::Provider;
use alloy::rpc::types::eth::TransactionRequest;
use ethers::types::{H256, U256};
use eyre::ContextCompat;
use futures::stream::FuturesUnordered;
use futures::StreamExt;

use crate::app::App;
use crate::broadcast_utils::should_send_relayer_transactions;
use crate::db::TxForEscalation;
use crate::types::RelayerInfo;

pub async fn escalate_txs_task(app: Arc<App>) -> eyre::Result<()> {
    loop {
        escalate_txs(&app).await?;

        tokio::time::sleep(app.config.service.escalation_interval).await;
    }
}

#[tracing::instrument(skip(app))]
async fn escalate_txs(app: &App) -> eyre::Result<()> {
    tracing::info!("Escalating transactions");

    let txs_for_escalation = app
        .db
        .get_txs_for_escalation(app.config.service.escalation_interval)
        .await?;

    tracing::info!("Got {} transactions to escalate", txs_for_escalation.len());

    let txs_for_escalation = split_txs_per_relayer(txs_for_escalation);

    let mut futures = FuturesUnordered::new();

    for (relayer_id, txs) in txs_for_escalation {
        futures.push(escalate_relayer_txs(app, relayer_id, txs));
    }

    while let Some(result) = futures.next().await {
        if let Err(err) = result {
            tracing::error!(error = ?err, "Failed escalating txs");
        }
    }

    Ok(())
}

#[tracing::instrument(skip(app, txs))]
async fn escalate_relayer_txs(
    app: &App,
    relayer_id: String,
    txs: Vec<TxForEscalation>,
) -> eyre::Result<()> {
    let relayer = app.db.get_relayer(&relayer_id).await?;

    if txs.is_empty() {
        tracing::info!("No transactions to escalate");
    }

    for tx in txs {
        escalate_relayer_tx(app, &relayer, tx).await?;
    }

    Ok(())
}

#[tracing::instrument(skip(app, relayer, tx), fields(tx_id = tx.id))]
async fn escalate_relayer_tx(
    app: &App,
    relayer: &RelayerInfo,
    tx: TxForEscalation,
) -> eyre::Result<()> {
    if !should_send_relayer_transactions(app, relayer).await? {
        tracing::warn!(relayer_id = relayer.id, "Skipping relayer escalations");

        return Ok(());
    }

    tracing::info!(
        tx_id = tx.id,
        escalation_count = tx.escalation_count,
        "Escalating transaction"
    );

    let escalation = tx.escalation_count + 1;

    let (provider, signer_address) = app
        .universal_provider(tx.chain_id, tx.key_id.clone())
        .await?;

    tracing::info!("Escalating transaction - got middleware");

    let fees = app
        .db
        .get_latest_block_fees_by_chain_id(tx.chain_id)
        .await?
        .context("Missing block")?;

    tracing::info!("Escalating transaction - got block fees");

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

    let to_alloy = tx.tx_to.0.to_fixed_bytes();
    let data: alloy::primitives::Bytes = tx.data.to_vec().into();
    let mut value = [0_u8; 32];
    tx.value.0.to_little_endian(&mut value);

    let mut tx_request = TransactionRequest::default()
        .with_from(signer_address)
        .with_to(alloy::primitives::Address::from_slice(&to_alloy))
        .with_gas_limit(tx.gas_limit.0.low_u128())
        .with_value(alloy::primitives::U256::from_le_slice(&value))
        .with_input(data)
        .with_nonce(tx.nonce)
        .with_access_list(alloy::eips::eip2930::AccessList::default())
        .with_max_priority_fee_per_gas(max_priority_fee_per_gas.low_u128())
        .with_max_fee_per_gas(max_fee_per_gas.low_u128())
        .with_chain_id(tx.chain_id);

    if let Some(blobs) = tx.blobs {
        let sidecar: SidecarBuilder<SimpleCoder> =
            SidecarBuilder::from_slice(&blobs[0]);

        let sidecar = sidecar.build()?;
        tx_request = tx_request
            .with_max_fee_per_blob_gas(max_fee_per_gas.low_u128())
            .with_blob_sidecar(sidecar);

        tx_request.populate_blob_hashes();
    }

    tracing::info!("Escalating transaction - assembled tx");

    let pending_tx = provider.send_transaction(tx_request).await;

    tracing::info!("Escalating transaction - sent tx");

    let pending_tx = match pending_tx {
        Ok(pending_tx) => pending_tx,
        Err(err) => {
            tracing::error!(tx_id = tx.id, error = ?err, "Failed to escalate transaction");
            return Ok(());
        }
    };

    tracing::info!("Escalating transaction - got pending tx");

    let tx_hash = pending_tx.tx_hash();

    tracing::info!(
        tx_id = tx.id,
        ?tx_hash,
        ?initial_max_priority_fee_per_gas,
        ?initial_max_fee_per_gas,
        ?max_priority_fee_per_gas,
        ?max_fee_per_gas,
        ?pending_tx,
        "Escalated transaction"
    );

    let db_tx_hash = H256::from_slice(&tx_hash.as_slice());

    app.db
        .escalate_tx(&tx.id, db_tx_hash, max_fee_per_gas, max_fee_per_gas)
        .await?;

    tracing::info!(tx_id = tx.id, "Escalated transaction saved");

    Ok(())
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
