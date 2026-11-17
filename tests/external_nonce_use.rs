mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use alloy::network::TransactionBuilder;
use alloy::providers::ProviderBuilder;
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use metrics::{
    Counter, Gauge, Histogram, Key, KeyName, Metadata, Recorder, SharedString,
    Unit,
};

use crate::common::prelude::*;

#[tokio::test]
async fn key_used_outside_tx_sitter_moves_nonce_forward() -> eyre::Result<()> {
    setup_tracing();
    let counters = CountingRecorder::install()?;

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let (_service, client) =
        ServiceBuilder::default().build(&anvil, &db_url).await?;
    let api_key = create_api_key(&client).await?;
    let db = connect_db(&db_url).await?;

    let relayer_key = PrivateKeySigner::from_slice(DEFAULT_ANVIL_PRIVATE_KEY)?;
    ProviderBuilder::new()
        .wallet(relayer_key)
        .connect_http(anvil.endpoint_url())
        .send_transaction(
            TransactionRequest::default()
                .with_to(ARBITRARY_ADDRESS)
                .with_value(U256::from(1)),
        )
        .await?
        .get_receipt()
        .await?;

    await_local_nonce(&db, 1).await?;
    assert_eq!(counters.get("relayer_nonce_behind_chain"), 1);

    let tx_id = send_transfer(&client, &api_key, U256::from(1)).await?;
    await_mined_tx_hash(&client, &api_key, &tx_id).await?;

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

/// A metrics recorder that counts counter increments by metric name.
#[derive(Clone, Default)]
struct CountingRecorder {
    counters: Arc<Mutex<HashMap<String, Arc<AtomicU64>>>>,
}

impl CountingRecorder {
    fn install() -> eyre::Result<Self> {
        let recorder = Self::default();
        metrics::set_global_recorder(recorder.clone())
            .map_err(|_| eyre::eyre!("A metrics recorder is installed"))?;

        Ok(recorder)
    }

    fn get(&self, name: &str) -> u64 {
        self.counters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .map_or(0, |counter| counter.load(Ordering::SeqCst))
    }
}

impl Recorder for CountingRecorder {
    fn describe_counter(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}

    fn describe_gauge(&self, _: KeyName, _: Option<Unit>, _: SharedString) {}

    fn describe_histogram(&self, _: KeyName, _: Option<Unit>, _: SharedString) {
    }

    fn register_counter(&self, key: &Key, _: &Metadata<'_>) -> Counter {
        let counter = self
            .counters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key.name().to_string())
            .or_default()
            .clone();

        Counter::from_arc(counter)
    }

    fn register_gauge(&self, _: &Key, _: &Metadata<'_>) -> Gauge {
        Gauge::noop()
    }

    fn register_histogram(&self, _: &Key, _: &Metadata<'_>) -> Histogram {
        Histogram::noop()
    }
}
