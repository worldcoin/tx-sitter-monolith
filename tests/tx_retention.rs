mod common;

use alloy::primitives::keccak256;
use tx_sitter::db::{Database, TxIdMatch};
use tx_sitter::types::{TransactionPriority, TxStatus};

use crate::common::prelude::*;

const TX_RETENTION: Duration = Duration::from_secs(60 * 60);
const MINUTE: Duration = Duration::from_secs(60);
const REQUEST_HASH: [u8; 32] = [7; 32];

#[tokio::test]
async fn prune_keeps_finalized_txs_within_retention() -> eyre::Result<()> {
    setup_tracing();

    let (db_url, _db_container) = setup_db().await?;
    let anvil = AnvilBuilder::default().spawn().await?;
    // Stores the anvil relayer without running tasks
    let (_api, _) = ServiceBuilder::default()
        .role(ServiceRole::Api)
        .build(&anvil, &db_url)
        .await?;
    let db = connect_db(&db_url).await?;

    create_finalized_tx(&db, "expired", TX_RETENTION + MINUTE).await?;
    create_finalized_tx(&db, "kept", TX_RETENTION - MINUTE).await?;

    // A worker prunes as it starts
    let (_worker, _) = ServiceBuilder::default()
        .role(ServiceRole::Worker)
        .tx_retention(TX_RETENTION)
        .build(&anvil, &db_url)
        .await?;
    await_pruned(&db, "expired").await?;

    let tx = db.read_tx("kept").await?;
    assert_eq!(tx.and_then(|tx| tx.status), Some(TxStatus::Finalized));
    for (tx_id, expected) in [
        ("kept", TxIdMatch::SameRequest),
        ("expired", TxIdMatch::Unused),
    ] {
        let tx_id_match = db
            .compare_tx_request(tx_id, DEFAULT_RELAYER_ID, REQUEST_HASH)
            .await?;
        assert_eq!(tx_id_match, expected, "{tx_id}");
    }

    Ok(())
}

/// Creates a transaction of the anvil relayer that was mined `mined_ago`
/// and is finalized
async fn create_finalized_tx(
    db: &Database,
    tx_id: &str,
    mined_ago: Duration,
) -> eyre::Result<()> {
    db.create_transaction(
        tx_id,
        ARBITRARY_ADDRESS,
        &[],
        U256::ZERO,
        U256::from(21_000),
        TransactionPriority::Regular,
        None,
        DEFAULT_RELAYER_ID,
        None,
        Some(REQUEST_HASH),
    )
    .await?;

    let fee = U256::from(1);
    db.insert_tx_broadcast(tx_id, keccak256(tx_id), &[], fee, fee)
        .await?;
    sqlx::query(
        r#"
        UPDATE sent_transactions
        SET    status = 'finalized',
               mined_at = now() - make_interval(secs => $2)
        WHERE  tx_id = $1
        "#,
    )
    .bind(tx_id)
    .bind(mined_ago.as_secs_f64())
    .execute(&db.pool)
    .await?;

    Ok(())
}

async fn await_pruned(db: &Database, tx_id: &str) -> eyre::Result<()> {
    for _ in 0..30 {
        if db.read_tx(tx_id).await?.is_none() {
            return Ok(());
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    eyre::bail!("Transaction {tx_id} was not pruned in time");
}
