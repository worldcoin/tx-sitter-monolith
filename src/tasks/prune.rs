use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use eyre::Context;

use crate::app::App;
use crate::task_runner::TaskContext;

const BLOCK_PRUNING_INTERVAL: Duration = Duration::from_secs(60);
const TX_PRUNING_INTERVAL: Duration = Duration::from_secs(60);

/// How many blocks or transactions one statement deletes at most, so that a
/// backlog doesn't turn into one statement too long to finish
const PRUNE_BATCH_SIZE: u64 = 1_000;

const fn minutes(minutes: i64) -> i64 {
    minutes * 60
}

const fn hours(hours: i64) -> i64 {
    minutes(hours) * 60
}

const fn days(days: i64) -> i64 {
    hours(days) * 24
}

// TODO: This should be a per network setting
//
// Finalized transactions may outlive their blocks, see `service.tx_retention`:
// reorg handling only looks at mined transactions, which finalize an hour
// after they are mined.
const BLOCK_PRUNE_AGE_SECONDS: i64 = days(7);

pub async fn prune_blocks(app: Arc<App>, ctx: TaskContext) -> eyre::Result<()> {
    while !ctx.is_shutting_down() {
        let prune_age = chrono::Duration::seconds(BLOCK_PRUNE_AGE_SECONDS);
        let block_prune_timestamp = Utc::now() - prune_age;

        tracing::info!(?block_prune_timestamp, "Pruning blocks");

        prune_in_batches(&ctx, |batch_size| {
            app.db.prune_blocks(block_prune_timestamp, batch_size)
        })
        .await?;

        ctx.idle(BLOCK_PRUNING_INTERVAL).await;
    }

    Ok(())
}

pub async fn prune_txs(app: Arc<App>, ctx: TaskContext) -> eyre::Result<()> {
    let retention = chrono::Duration::from_std(app.config.service.tx_retention)
        .context("service.tx_retention is out of range")?;

    while !ctx.is_shutting_down() {
        let tx_prune_timestamp = Utc::now() - retention;

        tracing::info!(?tx_prune_timestamp, "Pruning txs");

        prune_in_batches(&ctx, |batch_size| {
            app.db.prune_txs(tx_prune_timestamp, batch_size)
        })
        .await?;

        ctx.idle(TX_PRUNING_INTERVAL).await;
    }

    Ok(())
}

/// Calls `prune_batch` with [`PRUNE_BATCH_SIZE`] until it prunes fewer rows
/// than that, or shutdown is requested, and returns how many rows it pruned.
async fn prune_in_batches<F, Fut>(
    ctx: &TaskContext,
    mut prune_batch: F,
) -> eyre::Result<u64>
where
    F: FnMut(u64) -> Fut,
    Fut: Future<Output = eyre::Result<u64>>,
{
    let mut pruned = 0;

    while !ctx.is_shutting_down() {
        let batch = prune_batch(PRUNE_BATCH_SIZE).await?;
        pruned += batch;
        ctx.report_progress();

        if batch < PRUNE_BATCH_SIZE {
            break;
        }
    }

    Ok(pruned)
}

#[cfg(test)]
mod tests {
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::test_utils::{setup_app, CHAIN_ID};

    const BACKLOG: u64 = 2 * PRUNE_BATCH_SIZE + 500;

    #[tokio::test]
    async fn prunes_a_backlog_in_batches() -> eyre::Result<()> {
        let (app, _db_container) =
            setup_app("http://127.0.0.1:1", "ws://127.0.0.1:1").await?;
        let relayer_id = uuid::Uuid::new_v4().to_string();
        app.db
            .create_relayer(
                &relayer_id,
                "relayer",
                CHAIN_ID,
                "key_id",
                alloy::primitives::Address::with_last_byte(1),
            )
            .await?;
        seed_finalized_txs(&app, &relayer_id).await?;
        let ctx = TaskContext::new(CancellationToken::new());
        let prune_timestamp = Utc::now() - chrono::Duration::days(1);

        let mut batches = 0;
        let pruned_txs = prune_in_batches(&ctx, |batch_size| {
            batches += 1;
            app.db.prune_txs(prune_timestamp, batch_size)
        })
        .await?;
        assert_eq!((pruned_txs, batches), (BACKLOG, 3));

        let mut batches = 0;
        let pruned_blocks = prune_in_batches(&ctx, |batch_size| {
            batches += 1;
            app.db.prune_blocks(prune_timestamp, batch_size)
        })
        .await?;
        assert_eq!((pruned_blocks, batches), (BACKLOG, 3));

        for table in [
            "transactions",
            "tx_hashes",
            "sent_transactions",
            "blocks",
            "block_txs",
        ] {
            let (rows,): (i64,) =
                sqlx::query_as(&format!("SELECT COUNT(*) FROM {table}"))
                    .fetch_one(&app.db.pool)
                    .await?;
            assert_eq!(rows, 0, "{table}");
        }

        Ok(())
    }

    /// Stores [`BACKLOG`] transactions finalized two days ago, each mined in
    /// a block of its own.
    async fn seed_finalized_txs(
        app: &App,
        relayer_id: &str,
    ) -> eyre::Result<()> {
        let backlog = BACKLOG as i64;
        let mut tx = app.db.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO transactions (id, tx_to, data, value, gas_limit, nonce, priority, relayer_id)
            SELECT 'tx_' || n, '\x01', '', '\x00', '\x00', n, 'regular', $2
            FROM   generate_series(1, $1) n
            "#,
        )
        .bind(backlog)
        .bind(relayer_id)
        .execute(tx.as_mut())
        .await?;
        sqlx::query(
            r#"
            INSERT INTO tx_hashes (tx_hash, tx_id, max_fee_per_gas, max_priority_fee_per_gas)
            SELECT int8send(n), 'tx_' || n, '\x00', '\x00'
            FROM   generate_series(1, $1) n
            "#,
        )
        .bind(backlog)
        .execute(tx.as_mut())
        .await?;
        sqlx::query(
            r#"
            INSERT INTO sent_transactions (tx_id, initial_max_fee_per_gas, initial_max_priority_fee_per_gas, mined_at, status, valid_tx_hash)
            SELECT 'tx_' || n, '\x00', '\x00', now() - interval '2 days', 'finalized', int8send(n)
            FROM   generate_series(1, $1) n
            "#,
        )
        .bind(backlog)
        .execute(tx.as_mut())
        .await?;
        sqlx::query(
            r#"
            INSERT INTO blocks (block_number, chain_id, timestamp)
            SELECT n, $2, now() - interval '2 days'
            FROM   generate_series(1, $1) n
            "#,
        )
        .bind(backlog)
        .bind(CHAIN_ID as i64)
        .execute(tx.as_mut())
        .await?;
        sqlx::query(
            r#"
            INSERT INTO block_txs (block_number, chain_id, tx_hash)
            SELECT n, $2, int8send(n)
            FROM   generate_series(1, $1) n
            "#,
        )
        .bind(backlog)
        .bind(CHAIN_ID as i64)
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;

        Ok(())
    }
}
