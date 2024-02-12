use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ethers::providers::Middleware;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::transaction::eip2930::AccessList;
use ethers::types::{Address, Eip1559TransactionRequest, NameOrAddress, H256};
use eyre::ContextCompat;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use itertools::Itertools;

use crate::app::App;
use crate::broadcast_utils::{
    calculate_gas_fees_from_estimates, should_send_relayer_transactions,
};
use crate::db::UnsentTx;

const NO_TXS_SLEEP_DURATION: Duration = Duration::from_secs(2);

pub async fn broadcast_txs(app: Arc<App>) -> eyre::Result<()> {
    loop {
        let txs = app.db.get_unsent_txs().await?;
        let num_txs = txs.len();

        let txs_by_relayer = sort_txs_by_relayer(txs);

        let mut futures = FuturesUnordered::new();

        for (relayer_id, txs) in txs_by_relayer {
            futures.push(broadcast_relayer_txs(&app, relayer_id, txs));
        }

        while let Some(result) = futures.next().await {
            if let Err(err) = result {
                tracing::error!(error = ?err, "Failed broadcasting transactions");
            }
        }

        if num_txs == 0 {
            tokio::time::sleep(NO_TXS_SLEEP_DURATION).await;
        }
    }
}

#[tracing::instrument(skip(app, txs))]
async fn broadcast_relayer_txs(
    app: &App,
    relayer_id: String,
    txs: Vec<UnsentTx>,
) -> eyre::Result<()> {
    if txs.is_empty() {
        return Ok(());
    }

    let relayer = app.db.get_relayer(&relayer_id).await?;

    if !should_send_relayer_transactions(app, &relayer).await? {
        tracing::warn!(relayer_id = relayer_id, "Skipping relayer broadcasts");

        return Ok(());
    }

    tracing::info!(
        relayer_id,
        num_txs = txs.len(),
        "Broadcasting relayer transactions"
    );

    for tx in txs {
        tracing::info!(tx_id = tx.id, nonce = tx.nonce, "Sending transaction");

        let middleware = app
            .signer_middleware(tx.chain_id, tx.key_id.clone())
            .await?;

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

        let mut typed_transaction =
            TypedTransaction::Eip1559(Eip1559TransactionRequest {
                from: None,
                to: Some(NameOrAddress::from(Address::from(tx.tx_to.0))),
                gas: Some(tx.gas_limit.0),
                value: Some(tx.value.0),
                data: Some(tx.data.into()),
                nonce: Some(tx.nonce.into()),
                access_list: AccessList::default(),
                max_priority_fee_per_gas: Some(max_priority_fee_per_gas),
                max_fee_per_gas: Some(max_fee_per_gas),
                chain_id: Some(tx.chain_id.into()),
            });

        // Fill and simulate the transaction
        middleware
            .fill_transaction(&mut typed_transaction, None)
            .await?;

        // Get the raw signed tx and derive the tx hash
        let raw_signed_tx = middleware
            .signer()
            .raw_signed_tx(&typed_transaction)
            .await?;
        let tx_hash = H256::from(ethers::utils::keccak256(&raw_signed_tx));

        tracing::debug!(tx_id = tx.id, "Saving transaction");
        app.db
            .insert_tx_broadcast(
                &tx.id,
                tx_hash,
                max_fee_per_gas,
                max_priority_fee_per_gas,
            )
            .await?;

        tracing::debug!(tx_id = tx.id, "Sending transaction");

        let pending_tx = middleware.send_raw_transaction(raw_signed_tx).await;

        let pending_tx = match pending_tx {
            Ok(pending_tx) => pending_tx,
            Err(err) => {
                tracing::error!(tx_id = tx.id, error = ?err, "Failed to send transaction");
                continue;
            }
        };

        tracing::info!(
            tx_id = tx.id,
            tx_nonce = tx.nonce,
            tx_hash = ?tx_hash,
            ?pending_tx,
            "Transaction broadcast"
        );
    }

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
