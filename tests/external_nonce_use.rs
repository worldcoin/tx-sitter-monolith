mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use alloy::network::TransactionBuilder;
use alloy::node_bindings::AnvilInstance;
use alloy::providers::ProviderBuilder;
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use metrics::{
    Counter, Gauge, Histogram, Key, KeyName, Metadata, Recorder, SharedString,
    Unit,
};

use crate::common::prelude::*;

const ESCALATION_INTERVAL: Duration = Duration::from_secs(2);

/// The relayer's key sends three transactions outside tx-sitter, using the
/// nonces of the two transactions tx-sitter has queued.
#[tokio::test]
async fn key_used_outside_tx_sitter_moves_nonce_forward() -> eyre::Result<()> {
    setup_tracing();
    let metrics = CountingRecorder::install()?;

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let (_api, client) = ServiceBuilder::default()
        .role(ServiceRole::Api)
        .build(&anvil, &db_url)
        .await?;
    sync_relayer_nonces(&db_url, &anvil, &[DEFAULT_ANVIL_ACCOUNT]).await?;
    let api_key = DEFAULT_API_KEY.to_string();
    let db = connect_db(&db_url).await?;

    for _ in 0..2 {
        send_transfer(&client, &api_key, U256::from(1)).await?;
    }
    send_transfers_outside_tx_sitter(&anvil, 3).await?;

    let (_worker, _) = ServiceBuilder::default()
        .role(ServiceRole::Worker)
        .escalation_interval(ESCALATION_INTERVAL)
        .build(&anvil, &db_url)
        .await?;

    await_local_nonce(&db, 3).await?;
    assert_eq!(metrics.counter("relayer_nonce_behind_chain"), 1);
    await_gauge(&metrics, "relayer_stale_txs", 2.0).await?;

    let send_failures = metrics.counter("tx_send_failures");
    tokio::time::sleep(ESCALATION_INTERVAL * 3).await;
    assert_eq!(
        metrics.counter("tx_send_failures"),
        send_failures,
        "Stale transactions were sent again"
    );

    let tx_id = send_transfer(&client, &api_key, U256::from(1)).await?;
    await_mined_tx_hash(&client, &api_key, &tx_id).await?;

    Ok(())
}

/// Sends `count` transfers with the relayer's key, one at a time, without
/// tx-sitter.
async fn send_transfers_outside_tx_sitter(
    anvil: &AnvilInstance,
    count: usize,
) -> eyre::Result<()> {
    let relayer_key = PrivateKeySigner::from_slice(DEFAULT_ANVIL_PRIVATE_KEY)?;
    let provider = ProviderBuilder::new()
        .wallet(relayer_key)
        .connect_http(anvil.endpoint_url());

    for _ in 0..count {
        provider
            .send_transaction(
                TransactionRequest::default()
                    .with_to(ARBITRARY_ADDRESS)
                    .with_value(U256::from(1)),
            )
            .await?
            .get_receipt()
            .await?;
    }

    Ok(())
}

async fn await_local_nonce(
    db: &tx_sitter::db::Database,
    nonce: u64,
) -> eyre::Result<()> {
    for _ in 0..30 {
        let relayer = db.get_relayer(DEFAULT_RELAYER_ID).await?;

        if relayer.is_some_and(|relayer| relayer.nonce >= nonce) {
            return Ok(());
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    eyre::bail!("Local nonce did not reach {nonce}");
}

async fn await_gauge(
    metrics: &CountingRecorder,
    name: &str,
    expected: f64,
) -> eyre::Result<()> {
    for _ in 0..60 {
        if metrics.gauge(name) == expected {
            return Ok(());
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    eyre::bail!("Gauge {name} did not reach {expected}");
}

/// A metrics recorder that keeps the value of every counter and gauge by
/// metric name.
#[derive(Clone, Default)]
struct CountingRecorder {
    counters: Arc<Mutex<HashMap<String, Arc<AtomicU64>>>>,
    gauges: Arc<Mutex<HashMap<String, Arc<AtomicU64>>>>,
}

impl CountingRecorder {
    fn install() -> eyre::Result<Self> {
        let recorder = Self::default();
        metrics::set_global_recorder(recorder.clone())
            .map_err(|_| eyre::eyre!("A metrics recorder is installed"))?;

        Ok(recorder)
    }

    fn counter(&self, name: &str) -> u64 {
        lookup(&self.counters, name)
            .map_or(0, |counter| counter.load(Ordering::SeqCst))
    }

    fn gauge(&self, name: &str) -> f64 {
        lookup(&self.gauges, name)
            .map_or(0.0, |gauge| f64::from_bits(gauge.load(Ordering::SeqCst)))
    }
}

fn lookup(
    metrics: &Mutex<HashMap<String, Arc<AtomicU64>>>,
    name: &str,
) -> Option<Arc<AtomicU64>> {
    metrics
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(name)
        .cloned()
}

fn register(
    metrics: &Mutex<HashMap<String, Arc<AtomicU64>>>,
    key: &Key,
) -> Arc<AtomicU64> {
    metrics
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(key.name().to_string())
        .or_default()
        .clone()
}

impl Recorder for CountingRecorder {
    fn describe_counter(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}

    fn describe_gauge(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}

    fn describe_histogram(&self, _: KeyName, _: Option<Unit>, _: SharedString) {
    }

    fn register_counter(&self, key: &Key, _: &Metadata<'_>) -> Counter {
        Counter::from_arc(register(&self.counters, key))
    }

    fn register_gauge(&self, key: &Key, _: &Metadata<'_>) -> Gauge {
        Gauge::from_arc(register(&self.gauges, key))
    }

    fn register_histogram(&self, _: &Key, _: &Metadata<'_>) -> Histogram {
        Histogram::noop()
    }
}
