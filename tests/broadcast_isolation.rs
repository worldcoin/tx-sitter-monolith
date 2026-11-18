mod common;

use std::time::Instant;

use tx_sitter::broadcast_utils::gas_estimation::FeesEstimate;
use tx_sitter::db::data::ChainNonces;
use tx_sitter::db::{CreateResult, Database};
use tx_sitter::types::TransactionPriority;

use crate::common::prelude::*;

const UNRESPONSIVE_CHAIN_ID: u64 = 2;
/// Every attempt to send to the unresponsive chain takes this long to fail
const RPC_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn unresponsive_chain_does_not_delay_other_relayers() -> eyre::Result<()>
{
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    let unresponsive_rpc = UnresponsiveRpc::spawn().await?;
    let unresponsive_relayer =
        TestRelayer::random(UNRESPONSIVE_CHAIN_ID).config;
    let (_service, client) = ServiceBuilder::default()
        .rpc_timeout(RPC_TIMEOUT)
        .network(unresponsive_rpc.network(UNRESPONSIVE_CHAIN_ID))
        .relayer(unresponsive_relayer.clone())
        .build(&anvil, &db_url)
        .await?;
    let api_key = DEFAULT_API_KEY.to_string();
    let db = connect_db(&db_url).await?;

    let stuck_tx_id =
        create_tx_on_unresponsive_chain(&db, &unresponsive_relayer).await?;
    await_stored_broadcast(&db, &stuck_tx_id).await?;

    let started = Instant::now();
    let tx_id = send_transfer(&client, &api_key, U256::from(1)).await?;
    await_mined_tx_hash(&client, &api_key, &tx_id).await?;

    assert!(
        started.elapsed() < RPC_TIMEOUT * 2,
        "Mined after {:?}",
        started.elapsed()
    );

    Ok(())
}

/// Creates a transaction of `relayer`, a relayer on the chain whose RPC
/// never answers, with the nonce sync and fee estimates the worker can't get
/// from that chain, so that broadcast signs it and then waits for the send.
async fn create_tx_on_unresponsive_chain(
    db: &Database,
    relayer: &RelayerConfig,
) -> eyre::Result<String> {
    let nonces = ChainNonces {
        latest: 0,
        pending: Some(0),
    };
    db.update_relayer_nonce(UNRESPONSIVE_CHAIN_ID, relayer.address, nonces)
        .await?;
    let gwei: U256 = parse_units("1", "gwei")?.into();
    db.save_block(1, UNRESPONSIVE_CHAIN_ID, chrono::Utc::now(), &[])
        .await?;
    db.save_block_fees(
        1,
        UNRESPONSIVE_CHAIN_ID,
        &FeesEstimate {
            base_fee_per_gas: gwei,
            percentile_fees: vec![gwei; 5],
        },
        gwei,
    )
    .await?;

    let tx_id = "stuck".to_string();
    let result = db
        .create_transaction(
            &tx_id,
            ARBITRARY_ADDRESS,
            &[],
            U256::from(1),
            U256::from(21_000),
            TransactionPriority::Regular,
            None,
            &relayer.id,
            None,
            None,
        )
        .await?;
    assert_eq!(result, CreateResult::Created);

    Ok(tx_id)
}

/// Waits until broadcast stored `tx_id`, after which it sends it.
async fn await_stored_broadcast(
    db: &Database,
    tx_id: &str,
) -> eyre::Result<()> {
    for _ in 0..20 {
        let (is_stored,): (bool,) = sqlx::query_as(
            "SELECT EXISTS (SELECT 1 FROM sent_transactions WHERE tx_id = $1)",
        )
        .bind(tx_id)
        .fetch_one(&db.pool)
        .await?;

        if is_stored {
            return Ok(());
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    eyre::bail!("Transaction {tx_id} was not broadcast in time");
}
