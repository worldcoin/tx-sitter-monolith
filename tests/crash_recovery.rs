//! A worker can stop between storing a signed transaction and sending it.
//! The tests store a transaction the way the worker does, without sending it,
//! and check that the next worker sends the stored bytes.

mod common;

use alloy::consensus::TxEip1559;
use alloy::primitives::{keccak256, Bytes, TxKind};
use alloy::signers::local::PrivateKeySigner;
use tx_sitter::db::Database;
use tx_sitter::keys::UniversalSigner;

use crate::common::prelude::*;

const ESCALATION_INTERVAL: Duration = Duration::from_secs(10);
const MAX_ESCALATIONS: usize = 100;
const GWEI: u128 = 1_000_000_000;

#[tokio::test]
async fn worker_sends_broadcast_stored_before_a_crash() -> eyre::Result<()> {
    setup_tracing();
    let setup = Setup::new().await?;

    let stored = sign_transfer(&setup.provider, setup.value, GWEI).await?;
    setup
        .db
        .insert_tx_broadcast(
            &setup.tx_id,
            stored.tx_hash,
            &stored.raw_signed_tx,
            stored.max_fee_per_gas,
            stored.max_priority_fee_per_gas,
        )
        .await?;

    let (_worker, _) = setup.start_worker(MAX_ESCALATIONS).await?;

    let mined =
        await_mined_tx_hash(&setup.client, &setup.api_key, &setup.tx_id)
            .await?;
    assert_eq!(mined.0, stored.tx_hash);
    assert_eq!(setup.escalation_count().await?, 0);
    setup.assert_next_nonce_is_mined().await?;

    Ok(())
}

/// The stored escalation is the last one allowed, so only re-sending it
/// gets the transaction mined.
#[tokio::test]
async fn worker_sends_last_escalation_stored_before_a_crash() -> eyre::Result<()>
{
    setup_tracing();
    let setup = Setup::new().await?;

    let original = sign_transfer(&setup.provider, setup.value, GWEI).await?;
    setup
        .db
        .insert_tx_broadcast(
            &setup.tx_id,
            original.tx_hash,
            &original.raw_signed_tx,
            original.max_fee_per_gas,
            original.max_priority_fee_per_gas,
        )
        .await?;
    let replacement =
        sign_transfer(&setup.provider, setup.value, 2 * GWEI).await?;
    let is_stored = setup
        .db
        .escalate_tx(
            &setup.tx_id,
            0,
            replacement.tx_hash,
            &replacement.raw_signed_tx,
            replacement.max_fee_per_gas,
            replacement.max_priority_fee_per_gas,
        )
        .await?;
    assert!(is_stored);

    let (_worker, _) = setup.start_worker(1).await?;

    let mined =
        await_mined_tx_hash(&setup.client, &setup.api_key, &setup.tx_id)
            .await?;
    assert_eq!(mined.0, replacement.tx_hash);
    assert_eq!(setup.escalation_count().await?, 1);
    setup.assert_next_nonce_is_mined().await?;

    Ok(())
}

/// An API-only service, so nothing is broadcast, with one transaction of
/// the predefined relayer at nonce 0.
struct Setup {
    db: Database,
    db_url: String,
    anvil: alloy::node_bindings::AnvilInstance,
    provider: RootProvider,
    client: Configuration,
    api_key: String,
    tx_id: String,
    value: U256,
    _api: tx_sitter::service::Service,
    _db_container: postgres_docker_utils::DockerContainerGuard,
}

impl Setup {
    async fn new() -> eyre::Result<Self> {
        let (db_url, db_container) = setup_db().await?;
        let anvil = AnvilBuilder::default().spawn().await?;
        let provider = setup_provider(anvil.endpoint()).await?;

        let (api, client) = ServiceBuilder::default()
            .role(ServiceRole::Api)
            .build(&anvil, &db_url)
            .await?;
        let api_key = create_api_key(&client).await?;
        let value = U256::from(1_000);
        let tx_id = send_transfer(&client, &api_key, value).await?;

        Ok(Self {
            db: connect_db(&db_url).await?,
            db_url,
            anvil,
            provider,
            client,
            api_key,
            tx_id,
            value,
            _api: api,
            _db_container: db_container,
        })
    }

    async fn start_worker(
        &self,
        max_escalations: usize,
    ) -> eyre::Result<(tx_sitter::service::Service, Configuration)> {
        ServiceBuilder::default()
            .role(ServiceRole::Worker)
            .escalation_interval(ESCALATION_INTERVAL)
            .max_escalations(max_escalations)
            .build(&self.anvil, &self.db_url)
            .await
    }

    async fn escalation_count(&self) -> eyre::Result<i64> {
        let (escalation_count,): (i64,) = sqlx::query_as(
            "SELECT escalation_count FROM sent_transactions WHERE tx_id = $1",
        )
        .bind(&self.tx_id)
        .fetch_one(&self.db.pool)
        .await?;

        Ok(escalation_count)
    }

    /// Sends the relayer's next transaction and waits for it to be mined,
    /// which it can't be if a nonce before it is missing.
    async fn assert_next_nonce_is_mined(&self) -> eyre::Result<()> {
        let tx_id =
            send_transfer(&self.client, &self.api_key, self.value).await?;
        await_mined_tx_hash(&self.client, &self.api_key, &tx_id).await?;

        let nonce = self
            .provider
            .get_transaction_count(DEFAULT_ANVIL_ACCOUNT)
            .await?;
        assert_eq!(nonce, 2);

        Ok(())
    }
}

/// A signed transfer from the predefined relayer at nonce 0, matching the
/// transaction [`Setup`] creates.
struct SignedTx {
    tx_hash: B256,
    raw_signed_tx: Bytes,
    max_fee_per_gas: U256,
    max_priority_fee_per_gas: U256,
}

async fn sign_transfer(
    provider: &RootProvider,
    value: U256,
    max_priority_fee_per_gas: u128,
) -> eyre::Result<SignedTx> {
    let max_fee_per_gas =
        2 * provider.get_gas_price().await? + max_priority_fee_per_gas;
    let signer = UniversalSigner::Local(PrivateKeySigner::from_slice(
        DEFAULT_ANVIL_PRIVATE_KEY,
    )?);

    let raw_signed_tx = signer
        .raw_signed_tx(TxEip1559 {
            chain_id: DEFAULT_ANVIL_CHAIN_ID,
            nonce: 0,
            gas_limit: 21_000,
            max_fee_per_gas,
            max_priority_fee_per_gas,
            to: TxKind::Call(ARBITRARY_ADDRESS),
            value,
            access_list: Default::default(),
            input: Default::default(),
        })
        .await?;

    Ok(SignedTx {
        tx_hash: keccak256(&raw_signed_tx),
        raw_signed_tx,
        max_fee_per_gas: U256::from(max_fee_per_gas),
        max_priority_fee_per_gas: U256::from(max_priority_fee_per_gas),
    })
}
