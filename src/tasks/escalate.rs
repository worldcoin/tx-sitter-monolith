use std::collections::HashMap;
use std::sync::Arc;

use alloy::consensus::TxEip1559;
use alloy::primitives::{keccak256, TxKind, U256};
use eyre::{Context, ContextCompat};
use futures::stream::FuturesUnordered;
use futures::StreamExt;

use crate::app::App;
use crate::broadcast_utils::{
    send_raw_tx, should_send_relayer_transactions, SendFailure, SendOutcome,
};
use crate::db::data::RelayerInfo;
use crate::db::TxForEscalation;
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

        escalate_relayer_tx(app, ctx, &relayer, tx).await?;
        ctx.report_progress();
    }

    Ok(())
}

/// Sends `tx` again with higher fees, unless it turns out not to need it.
///
/// First the stored bytes of its latest version are sent again. A node that
/// didn't have them, because their send failed or the process stopped before
/// it, takes them now, and the fees are left alone until the next escalation.
/// A node that is unavailable or reports the nonce as used also skips the
/// escalation. Otherwise the replacement is signed and stored, and only sent
/// if no concurrent escalation stored one first.
#[tracing::instrument(
    skip(app, ctx, relayer, tx),
    fields(
        relayer_id = relayer.id,
        relayer_address = %relayer.address,
        tx_id = tx.id
    )
)]
async fn escalate_relayer_tx(
    app: &App,
    ctx: &TaskContext,
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

    let provider = app.http_provider(tx.chain_id).await?;

    if let Some(raw_signed_tx) = &tx.raw_signed_tx {
        match send_raw_tx(&provider, ctx, relayer, &tx.id, raw_signed_tx).await
        {
            SendOutcome::Accepted => {
                tracing::warn!(
                    relayer_id = relayer.id,
                    relayer_address = %relayer.address,
                    tx_id = tx.id,
                    "Re-sent a transaction the node didn't have"
                );
                return Ok(());
            }
            SendOutcome::Failed(SendFailure::NonceTooLow) => return Ok(()),
            SendOutcome::Failed(failure) if failure.is_retryable() => {
                return Ok(())
            }
            SendOutcome::AlreadyKnown | SendOutcome::Failed(_) => {}
        }
    }

    tracing::info!(
        relayer_id = relayer.id,
        relayer_address = %relayer.address,
        tx_id = tx.id,
        escalation_count = tx.escalation_count,
        "Escalating transaction"
    );

    let escalation = tx.escalation_count + 1;

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

    let raw_signed_tx = signer.raw_signed_tx(eip1559_tx).await?;
    let tx_hash = keccak256(&raw_signed_tx);

    let is_stored = app
        .db
        .escalate_tx(
            &tx.id,
            tx.escalation_count,
            tx_hash,
            &raw_signed_tx,
            max_fee_per_gas,
            max_priority_fee_per_gas,
        )
        .await?;

    if !is_stored {
        tracing::warn!(
            relayer_id = relayer.id,
            relayer_address = %relayer.address,
            tx_id = tx.id,
            escalation_count = tx.escalation_count,
            "Transaction changed while escalating it, dropping the escalation"
        );
        metrics::counter!(
            "tx_escalation_conflicts",
            &[("chain_id", tx.chain_id.to_string())]
        )
        .increment(1);

        return Ok(());
    }

    send_raw_tx(&provider, ctx, relayer, &tx.id, &raw_signed_tx).await;

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

    if escalation >= app.config.service.max_escalations {
        tracing::warn!(
            relayer_id = relayer.id,
            relayer_address = %relayer.address,
            tx_id = tx.id,
            escalation_count = escalation,
            "Too many escalations"
        );
    }

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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use alloy::node_bindings::{Anvil, AnvilInstance};
    use alloy::primitives::{Address, Bytes, B256};
    use alloy::providers::{Provider, RootProvider};
    use alloy::signers::local::PrivateKeySigner;
    use postgres_docker_utils::DockerContainerGuard;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::broadcast_utils::gas_estimation::FeesEstimate;
    use crate::keys::UniversalSigner;
    use crate::test_utils::{setup_app, CHAIN_ID};
    use crate::types::TransactionPriority;

    const GWEI: u64 = 1_000_000_000;
    const TX_ID: &str = "tx_id";

    #[tokio::test]
    async fn concurrent_escalations_send_one_replacement() -> eyre::Result<()> {
        let setup = Setup::new().await?;
        let original = setup.broadcast(SendsTx::Yes).await?;

        let tx = setup.tx_for_escalation().await?;
        let ctx = ctx();
        tokio::try_join!(
            escalate_relayer_tx(&setup.app, &ctx, &setup.relayer, tx.clone()),
            escalate_relayer_tx(&setup.app, &ctx, &setup.relayer, tx),
        )?;

        let (escalation_count, hashes) = setup.stored_escalations().await?;
        assert_eq!(escalation_count, 1);
        assert_eq!(hashes.len(), 2);
        assert_eq!(hashes[0], original);
        assert!(setup.is_pending_on_node(hashes[1]).await?);
        assert!(!setup.is_pending_on_node(original).await?);

        Ok(())
    }

    #[tokio::test]
    async fn sends_stored_bytes_before_bumping_fees() -> eyre::Result<()> {
        let setup = Setup::new().await?;
        let original = setup.broadcast(SendsTx::No).await?;

        let tx = setup.tx_for_escalation().await?;
        escalate_relayer_tx(&setup.app, &ctx(), &setup.relayer, tx).await?;

        assert_eq!(setup.stored_escalations().await?, (0, vec![original]));
        assert!(setup.is_pending_on_node(original).await?);

        let tx = setup.tx_for_escalation().await?;
        escalate_relayer_tx(&setup.app, &ctx(), &setup.relayer, tx).await?;

        let (escalation_count, hashes) = setup.stored_escalations().await?;
        assert_eq!(escalation_count, 1);
        assert!(setup.is_pending_on_node(hashes[1]).await?);

        Ok(())
    }

    fn ctx() -> TaskContext {
        TaskContext::new(CancellationToken::new())
    }

    enum SendsTx {
        Yes,
        No,
    }

    /// An app on an anvil node that only mines on request, with a relayer
    /// that has one transaction.
    struct Setup {
        app: App,
        relayer: RelayerInfo,
        provider: RootProvider,
        signer: UniversalSigner,
        _anvil: AnvilInstance,
        _db_container: DockerContainerGuard,
    }

    impl Setup {
        async fn new() -> eyre::Result<Self> {
            let anvil = Anvil::new()
                .chain_id(CHAIN_ID)
                .arg("--no-mining")
                .try_spawn()?;
            let (app, db_container) =
                setup_app(&anvil.endpoint(), &anvil.ws_endpoint()).await?;

            let key = anvil.keys()[0].clone();
            let relayer_id = uuid::Uuid::new_v4().to_string();
            app.db
                .create_relayer(
                    &relayer_id,
                    "relayer",
                    CHAIN_ID,
                    &hex::encode(key.to_bytes()),
                    anvil.addresses()[0],
                )
                .await?;
            let relayer = app
                .db
                .get_relayer(&relayer_id)
                .await?
                .context("Missing relayer")?;

            app.db
                .save_block(1, CHAIN_ID, chrono::Utc::now(), &[])
                .await?;
            app.db
                .save_block_fees(
                    1,
                    CHAIN_ID,
                    &FeesEstimate {
                        base_fee_per_gas: U256::from(GWEI),
                        percentile_fees: vec![U256::from(GWEI); 5],
                    },
                    U256::from(GWEI),
                )
                .await?;

            app.db
                .create_transaction(
                    TX_ID,
                    Address::with_last_byte(1),
                    &[],
                    U256::from(1),
                    U256::from(21_000),
                    TransactionPriority::Regular,
                    None,
                    &relayer_id,
                )
                .await?;

            Ok(Self {
                provider: RootProvider::new_http(anvil.endpoint_url()),
                signer: UniversalSigner::Local(PrivateKeySigner::from(key)),
                app,
                relayer,
                _anvil: anvil,
                _db_container: db_container,
            })
        }

        /// Stores the first signed version of the transaction, as broadcast
        /// does, and returns its hash.
        async fn broadcast(&self, sends_tx: SendsTx) -> eyre::Result<B256> {
            let (max_fee_per_gas, max_priority_fee_per_gas) = (2 * GWEI, GWEI);
            let raw_signed_tx: Bytes = self
                .signer
                .raw_signed_tx(TxEip1559 {
                    chain_id: CHAIN_ID,
                    nonce: 0,
                    gas_limit: 21_000,
                    max_fee_per_gas: max_fee_per_gas.into(),
                    max_priority_fee_per_gas: max_priority_fee_per_gas.into(),
                    to: TxKind::Call(Address::with_last_byte(1)),
                    value: U256::from(1),
                    access_list: Default::default(),
                    input: Default::default(),
                })
                .await?;
            let tx_hash = keccak256(&raw_signed_tx);

            self.app
                .db
                .insert_tx_broadcast(
                    TX_ID,
                    tx_hash,
                    &raw_signed_tx,
                    U256::from(max_fee_per_gas),
                    U256::from(max_priority_fee_per_gas),
                )
                .await?;

            if let SendsTx::Yes = sends_tx {
                let _ =
                    self.provider.send_raw_transaction(&raw_signed_tx).await?;
            }

            Ok(tx_hash)
        }

        async fn tx_for_escalation(&self) -> eyre::Result<TxForEscalation> {
            let mut txs = self
                .app
                .db
                .get_txs_for_escalation(Duration::ZERO, 100)
                .await?;
            assert_eq!(txs.len(), 1);

            Ok(txs.remove(0))
        }

        /// Returns the escalation count and the stored hashes, oldest first.
        async fn stored_escalations(&self) -> eyre::Result<(i64, Vec<B256>)> {
            let (escalation_count,): (i64,) = sqlx::query_as(
                "SELECT escalation_count FROM sent_transactions WHERE tx_id = $1",
            )
            .bind(TX_ID)
            .fetch_one(&self.app.db.pool)
            .await?;
            let hashes: Vec<(Vec<u8>,)> = sqlx::query_as(
                "SELECT tx_hash FROM tx_hashes WHERE tx_id = $1 ORDER BY created_at",
            )
            .bind(TX_ID)
            .fetch_all(&self.app.db.pool)
            .await?;

            Ok((
                escalation_count,
                hashes
                    .into_iter()
                    .map(|(hash,)| B256::from_slice(&hash))
                    .collect(),
            ))
        }

        async fn is_pending_on_node(
            &self,
            tx_hash: B256,
        ) -> eyre::Result<bool> {
            Ok(self
                .provider
                .get_transaction_by_hash(tx_hash)
                .await?
                .is_some())
        }
    }
}
