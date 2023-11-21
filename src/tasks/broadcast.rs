use std::sync::Arc;
use std::time::Duration;

use ethers::providers::Middleware;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::transaction::eip2930::AccessList;
use ethers::types::{Address, Eip1559TransactionRequest, NameOrAddress};
use eyre::ContextCompat;

use crate::app::App;
use crate::broadcast_utils::{
    calculate_gas_fees_from_estimates, calculate_max_base_fee_per_gas,
};

const MAX_IN_FLIGHT_TXS: usize = 5;

pub async fn broadcast_txs(app: Arc<App>) -> eyre::Result<()> {
    loop {
        let txs = app.db.get_unsent_txs(MAX_IN_FLIGHT_TXS).await?;

        // TODO: Parallelize per chain id?
        for tx in txs {
            tracing::info!(tx.id, "Sending tx");

            let middleware = app
                .fetch_signer_middleware(tx.chain_id, tx.key_id.clone())
                .await?;

            let estimates = app
                .db
                .get_latest_block_fees_by_chain_id(tx.chain_id)
                .await?
                .context("Missing block")?;

            let max_base_fee_per_gas =
                calculate_max_base_fee_per_gas(&estimates)?;

            let (max_fee_per_gas, max_priority_fee_per_gas) =
                calculate_gas_fees_from_estimates(
                    &estimates,
                    2, // Priority - 50th percentile
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
                .await?;

            let tx_hash = pending_tx.tx_hash();

            tracing::info!(?tx.id, ?tx_hash, "Tx sent successfully");

            app.db
                .insert_tx_broadcast(
                    &tx.id,
                    tx_hash,
                    max_fee_per_gas,
                    max_priority_fee_per_gas,
                )
                .await?;
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
