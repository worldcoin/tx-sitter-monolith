use std::sync::Arc;

use ethers::providers::Middleware;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::transaction::eip2930::AccessList;
use ethers::types::{Address, Eip1559TransactionRequest, NameOrAddress, U256};
use eyre::ContextCompat;

use crate::app::App;
use crate::broadcast_utils::should_send_transaction;

pub async fn escalate_txs(app: Arc<App>) -> eyre::Result<()> {
    loop {
        let txs_for_escalation = app
            .db
            .get_txs_for_escalation(app.config.service.escalation_interval)
            .await?;

        for tx in txs_for_escalation {
            tracing::info!(id = tx.id, tx.escalation_count, "Escalating tx");

            if !should_send_transaction(&app, &tx.relayer_id).await? {
                tracing::warn!(id = tx.id, "Skipping transaction broadcast");
                continue;
            }

            let escalation = tx.escalation_count + 1;

            let middleware = app
                .signer_middleware(tx.chain_id, tx.key_id.clone())
                .await?;

            let fees = app
                .db
                .get_latest_block_fees_by_chain_id(tx.chain_id)
                .await?
                .context("Missing block")?;

            // Min increase of 20% on the priority fee required for a replacement tx
            let factor = U256::from(100);
            let increased_gas_price_percentage =
                factor + U256::from(10 * (1 + escalation));

            let max_fee_per_gas_increase = tx.initial_max_fee_per_gas.0
                * increased_gas_price_percentage
                / factor;

            let max_fee_per_gas =
                tx.initial_max_fee_per_gas.0 + max_fee_per_gas_increase;

            let max_priority_fee_per_gas =
                max_fee_per_gas - fees.fee_estimates.base_fee_per_gas;

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
                .escalate_tx(
                    &tx.id,
                    tx_hash,
                    max_fee_per_gas,
                    max_priority_fee_per_gas,
                )
                .await?;

            tracing::info!(id = ?tx.id, hash = ?tx_hash, "Tx escalated");
        }

        tokio::time::sleep(app.config.service.escalation_interval).await;
    }
}
