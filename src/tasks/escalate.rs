use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

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
        .get_txs_for_escalation(app.config.service.escalation_interval)
        .await?;

    tracing::info!("Got {} transactions to escalate", txs_for_escalation.len());

    report_txs_at_max_escalations(app).await?;

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

/// Sets the `relayer_txs_at_max_escalations` gauge of every relayer, which
/// stays raised while a transaction whose fees are no longer bumped is
/// pending.
async fn report_txs_at_max_escalations(app: &App) -> eyre::Result<()> {
    let relayers = app
        .db
        .count_txs_at_max_escalations(app.config.service.max_escalations)
        .await?;

    for relayer in relayers {
        metrics::gauge!(
            "relayer_txs_at_max_escalations",
            &[
                ("relayer_id", relayer.relayer_id),
                ("chain_id", relayer.chain_id.to_string()),
            ]
        )
        .set(relayer.txs as f64);
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

/// Sends the latest signed version of `tx` again, then replaces it with a
/// higher-fee version if [`should_bump_fees`] says so and `tx` has fewer than
/// `max_escalations` escalations.
///
/// Sending the stored bytes again recovers a transaction whose last send
/// failed, or whose process stopped before sending it, however often it was
/// escalated. The replacement is signed and stored, and only sent if no
/// concurrent escalation stored one first.
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

    let resent = match &tx.raw_signed_tx {
        Some(raw_signed_tx) => Some(
            send_raw_tx(&provider, ctx, relayer, &tx.id, raw_signed_tx).await,
        ),
        None => None,
    };

    if resent == Some(SendOutcome::Accepted) {
        metrics::counter!(
            "tx_resend_accepted",
            &[("chain_id", tx.chain_id.to_string())]
        )
        .increment(1);
    }

    let max_escalations = app.config.service.max_escalations;
    let latest_hash_age = Duration::from_millis(tx.latest_hash_age_ms);

    if tx.escalation_count >= max_escalations
        || !should_bump_fees(
            resent,
            latest_hash_age,
            app.config.service.escalation_interval,
        )
    {
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

    let outcome =
        send_raw_tx(&provider, ctx, relayer, &tx.id, &raw_signed_tx).await;

    if let SendOutcome::Accepted | SendOutcome::AlreadyKnown = outcome {
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
    }

    if escalation == max_escalations {
        tracing::error!(
            relayer_id = relayer.id,
            relayer_address = %relayer.address,
            chain_id = tx.chain_id,
            tx_id = tx.id,
            escalation_count = escalation,
            "Transaction reached max escalations, its fees are no longer bumped"
        );
        metrics::counter!(
            "tx_escalation_limit_reached",
            &[("chain_id", tx.chain_id.to_string())]
        )
        .increment(1);
    }

    Ok(())
}

/// Decides whether a pending transaction needs a higher-fee replacement,
/// given what came of sending its latest version again: `None` if that
/// version has no stored bytes.
///
/// A node that only now took the latest version, because it lost it or
/// never got it, gets until the version is two escalation intervals old
/// before the fees go up. That defers each bump at most once, also for nodes
/// that answer every send as if it were new. A relayer that can't pay for the
/// transaction, fees above the node's cap, an unavailable node or a used
/// nonce leave the fees alone, because the replacement would fail the same
/// way. Any other rejection bumps them, as far as `max_escalations` allows.
fn should_bump_fees(
    resent: Option<SendOutcome>,
    latest_hash_age: Duration,
    escalation_interval: Duration,
) -> bool {
    let failure = match resent {
        None | Some(SendOutcome::AlreadyKnown) => return true,
        Some(SendOutcome::Accepted) => {
            return latest_hash_age >= 2 * escalation_interval;
        }
        Some(SendOutcome::Failed(failure)) => failure,
    };

    match failure {
        SendFailure::Underpriced | SendFailure::Rejected => true,
        SendFailure::InsufficientFunds
        | SendFailure::FeeCapExceeded
        | SendFailure::NonceTooLow
        | SendFailure::UnexpectedHash
        | SendFailure::Timeout
        | SendFailure::Connection
        | SendFailure::RateLimited
        | SendFailure::ServerError => false,
    }
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
    use alloy::node_bindings::{Anvil, AnvilInstance};
    use alloy::primitives::{Address, Bytes, B256};
    use alloy::providers::{Provider, RootProvider};
    use alloy::signers::local::PrivateKeySigner;
    use postgres_docker_utils::DockerContainerGuard;
    use test_case::test_case;
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

    #[tokio::test]
    async fn bumps_fees_of_a_tx_the_node_keeps_dropping() -> eyre::Result<()> {
        let setup = Setup::new().await?;
        let original = setup.broadcast(SendsTx::Yes).await?;
        let interval = setup.app.config.service.escalation_interval;

        for (latest_hash_age, expected_escalation_count) in
            [(interval * 3 / 2, 0), (interval * 5 / 2, 1)]
        {
            setup.drop_from_node(original).await?;
            setup.age_latest_hash(latest_hash_age).await?;

            let tx = setup.tx_for_escalation().await?;
            escalate_relayer_tx(&setup.app, &ctx(), &setup.relayer, tx).await?;

            let (escalation_count, hashes) = setup.stored_escalations().await?;
            assert_eq!(escalation_count, expected_escalation_count);
            assert!(setup.is_pending_on_node(*hashes.last().unwrap()).await?);
        }

        Ok(())
    }

    #[test_case(None, 1 => true ; "without stored bytes")]
    #[test_case(Some(SendOutcome::AlreadyKnown), 1 => true ; "known to the node")]
    #[test_case(Some(SendOutcome::Accepted), 1 => false ; "taken by the node only now")]
    #[test_case(Some(SendOutcome::Accepted), 2 => true ; "taken by the node only now two intervals after storing")]
    #[test_case(Some(SendOutcome::Failed(SendFailure::Underpriced)), 1 => true ; "underpriced")]
    #[test_case(Some(SendOutcome::Failed(SendFailure::Rejected)), 1 => true ; "rejected for another reason")]
    #[test_case(Some(SendOutcome::Failed(SendFailure::InsufficientFunds)), 9 => false ; "insufficient funds")]
    #[test_case(Some(SendOutcome::Failed(SendFailure::FeeCapExceeded)), 9 => false ; "above the fee cap")]
    #[test_case(Some(SendOutcome::Failed(SendFailure::UnexpectedHash)), 9 => false ; "unexpected hash")]
    #[test_case(Some(SendOutcome::Failed(SendFailure::Timeout)), 9 => false ; "node unavailable")]
    #[test_case(Some(SendOutcome::Failed(SendFailure::NonceTooLow)), 9 => false ; "nonce used")]
    fn bumps_fees(
        resent: Option<SendOutcome>,
        latest_hash_age_in_intervals: u32,
    ) -> bool {
        let interval = Duration::from_secs(60);

        should_bump_fees(
            resent,
            interval * latest_hash_age_in_intervals,
            interval,
        )
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
                    None,
                    None,
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
            let mut txs =
                self.app.db.get_txs_for_escalation(Duration::ZERO).await?;
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

        /// Makes the node forget `tx_hash`, as a node that evicts it from its
        /// pool does.
        async fn drop_from_node(&self, tx_hash: B256) -> eyre::Result<()> {
            self.provider
                .raw_request::<_, serde_json::Value>(
                    "anvil_dropTransaction".into(),
                    (tx_hash,),
                )
                .await?;

            Ok(())
        }

        /// Moves the time the latest version was stored back to
        /// `latest_hash_age` ago.
        async fn age_latest_hash(
            &self,
            latest_hash_age: Duration,
        ) -> eyre::Result<()> {
            sqlx::query(
                "UPDATE tx_hashes SET created_at = now() - $2 WHERE tx_id = $1 AND NOT escalated",
            )
            .bind(TX_ID)
            .bind(latest_hash_age)
            .execute(&self.app.db.pool)
            .await?;

            Ok(())
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
