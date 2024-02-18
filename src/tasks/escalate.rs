use std::sync::Arc;
use tracing::instrument;

use ethers::providers::Middleware;
use ethers::types::transaction::eip2718::TypedTransaction;
use ethers::types::transaction::eip2930::AccessList;
use ethers::types::{Address, Eip1559TransactionRequest, NameOrAddress, U256};
use eyre::ContextCompat;

use crate::app::App;

#[instrument(skip(app), name= "escalate")]
pub async fn escalate_txs(app: Arc<App>) -> eyre::Result<()> {
    loop {
        escalate_txs_run(app.clone()).await?;
        tokio::time::sleep(app.config.service.escalation_interval).await;
    }
}

#[instrument(skip(app), name= "escalate_run")]
async fn escalate_txs_run(app: Arc<App>) -> eyre::Result<()> {
    let txs_for_escalation = app
        .db
        .fetch_txs_for_escalation(app.config.service.escalation_interval)
        .await?;

    for tx in txs_for_escalation {
        tracing::info!(tx.id, "Escalating tx");

        // does it feel like we could cache it here (in case lots of transactions?)
        let middleware = app
            .fetch_signer_middleware(tx.chain_id, tx.key_id.clone())
            .await?;

        let escalation = tx.escalation_count + 1;

        let estimates = app
            .db
            .get_latest_block_fees_by_chain_id(tx.chain_id)
            .await?
            .context("Missing block")?;

        // Min increase of 20% on the priority fee required for a replacement tx
        let increased_gas_price_percentage =
            U256::from(100 + (10 * (1 + escalation)));

        let factor = U256::from(100);

        let max_priority_fee_per_gas_increase =
            tx.initial_max_priority_fee_per_gas.0
                * increased_gas_price_percentage
                / factor;

        // TODO: Add limits per network
        let max_priority_fee_per_gas =
            tx.initial_max_priority_fee_per_gas.0
                + max_priority_fee_per_gas_increase;

        let max_fee_per_gas =
            estimates.base_fee_per_gas + max_priority_fee_per_gas;

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
            .await?;

        let tx_hash = pending_tx.tx_hash();

        tracing::info!(?tx.id, ?tx_hash, "Tx escalated");

        app.db
            .escalate_tx(
                &tx.id,
                tx_hash,
                max_fee_per_gas,
                max_priority_fee_per_gas,
            )
            .await?;
    }

    Ok(())
}