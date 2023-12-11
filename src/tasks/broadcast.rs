use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ethers::providers::Middleware;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::transaction::eip2930::AccessList;
use ethers::types::{Address, Eip1559TransactionRequest, NameOrAddress};
use eyre::ContextCompat;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use itertools::Itertools;

use crate::app::App;
use crate::broadcast_utils::{
    calculate_gas_fees_from_estimates, calculate_max_base_fee_per_gas,
    should_send_transaction,
};
use crate::db::UnsentTx;

pub async fn broadcast_txs(app: Arc<App>) -> eyre::Result<()> {
    loop {
        let mut txs = app.db.get_unsent_txs().await?;

        txs.sort_unstable_by_key(|tx| tx.relayer_id.clone());

        let txs_by_relayer =
            txs.into_iter().group_by(|tx| tx.relayer_id.clone());

        let txs_by_relayer: HashMap<_, _> = txs_by_relayer
            .into_iter()
            .map(|(relayer_id, txs)| {
                let mut txs = txs.collect_vec();

                txs.sort_unstable_by_key(|tx| tx.nonce);

                (relayer_id, txs)
            })
            .collect();

        let mut futures = FuturesUnordered::new();

        for (relayer_id, txs) in txs_by_relayer {
            futures.push(broadcast_relayer_txs(&app, relayer_id, txs));
        }

        while let Some(result) = futures.next().await {
            if let Err(err) = result {
                tracing::error!(error = ?err, "Failed broadcasting txs");
            }
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[tracing::instrument(skip(app, txs))]
async fn broadcast_relayer_txs(
    app: &App,
    relayer_id: String,
    txs: Vec<UnsentTx>,
) -> Result<(), eyre::Error> {
    if txs.is_empty() {
        return Ok(());
    }

    tracing::info!(relayer_id, num_txs = txs.len(), "Broadcasting relayer txs");

    if !should_send_transaction(app, &relayer_id).await? {
        tracing::warn!(
            relayer_id = relayer_id,
            "Skipping transaction broadcasts"
        );

        return Ok(());
    }

    for tx in txs {
        tracing::info!(id = tx.id, "Sending tx");

        let middleware = app
            .signer_middleware(tx.chain_id, tx.key_id.clone())
            .await?;

        let fees = app
            .db
            .get_latest_block_fees_by_chain_id(tx.chain_id)
            .await?
            .context("Missing block fees")?;

        let max_base_fee_per_gas =
            calculate_max_base_fee_per_gas(&fees.fee_estimates)?;

        let (max_fee_per_gas, max_priority_fee_per_gas) =
            calculate_gas_fees_from_estimates(
                &fees.fee_estimates,
                tx.priority.to_percentile_index(),
                max_base_fee_per_gas,
            )?;

        let eip1559_tx = Eip1559TransactionRequest {
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
        };

        tracing::debug!(?eip1559_tx, "Sending tx");

        // TODO: Is it possible that we send a tx but don't store it in the DB?
        // TODO: Be smarter about error handling - a tx can fail to be sent
        //       e.g. because the relayer is out of funds
        //       but we don't want to retry it forever
        let pending_tx = middleware
            .send_transaction(TypedTransaction::Eip1559(eip1559_tx), None)
            .await;

        let pending_tx = match pending_tx {
            Ok(pending_tx) => {
                tracing::info!(?pending_tx, "Tx sent successfully");
                pending_tx
            }
            Err(err) => {
                tracing::error!(error = ?err, "Failed to send tx");
                continue;
            }
        };

        let tx_hash = pending_tx.tx_hash();

        app.db
            .insert_tx_broadcast(
                &tx.id,
                tx_hash,
                max_fee_per_gas,
                max_priority_fee_per_gas,
            )
            .await?;

        tracing::info!(id = tx.id, hash = ?tx_hash, "Tx broadcast");
    }

    Ok(())
}
