mod common;

use alloy::primitives::keccak256;
use tx_sitter::db::Database;

use crate::common::prelude::*;

const TX_RETENTION: Duration = Duration::from_secs(60 * 60);
const MINUTE: Duration = Duration::from_secs(60);

#[tokio::test]
async fn sends_stay_idempotent_while_their_tx_is_kept() -> eyre::Result<()> {
    setup_tracing();

    // Stores the anvil relayer without running tasks
    let service =
        TestService::spawn(ServiceBuilder::default().role(ServiceRole::Api))
            .await?;
    let client = service.v2_client(&service.api_key);
    let db = &service.db;

    for (tx_id, mined_ago) in [
        ("expired", TX_RETENTION + MINUTE),
        ("kept", TX_RETENTION - MINUTE),
    ] {
        send_tx_v2(&client, send_tx_request(tx_id)).await?;
        finalize(db, tx_id, mined_ago).await?;
    }
    assert_eq!(service.relayer_nonce().await?, 2);

    // A worker prunes as it starts
    let (_worker, _) = ServiceBuilder::default()
        .role(ServiceRole::Worker)
        .tx_retention(TX_RETENTION)
        .build(&service.anvil, &service.db_url)
        .await?;
    await_pruned(db, "expired").await?;

    let kept = get_tx_v2(&client, "kept").await?;
    assert_eq!(kept.status, TxStatusV2::Finalized);
    send_tx_v2(&client, send_tx_request("kept")).await?;
    assert_eq!(service.relayer_nonce().await?, 2, "A kept tx was repeated");

    // Once its transaction is pruned, the id is free again
    send_tx_v2(&client, send_tx_request("expired")).await?;
    let expired = get_tx_v2(&client, "expired").await?;
    assert_eq!(expired.status, TxStatusV2::Queued);
    assert_eq!(expired.nonce, 2);

    Ok(())
}

fn send_tx_request(tx_id: &str) -> SendTxRequestV2 {
    SendTxRequestV2 {
        tx_id: Some(tx_id.to_string()),
        ..SendTxRequestV2::new(
            ARBITRARY_ADDRESS.into(),
            U256::ZERO.into(),
            U256::from(21_000).into(),
        )
    }
}

/// Marks `tx_id` as broadcast and finalized, mined `mined_ago`
async fn finalize(
    db: &Database,
    tx_id: &str,
    mined_ago: Duration,
) -> eyre::Result<()> {
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
