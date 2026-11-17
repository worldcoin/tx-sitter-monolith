#![allow(clippy::too_many_arguments)]

use std::time::Duration;

use alloy::primitives::{Address, B256, U256};
use chrono::{DateTime, Utc};
use sqlx::migrate::{MigrateDatabase, Migrator};
use sqlx::postgres::PgPoolOptions;
use sqlx::types::{BigDecimal, Json};
use sqlx::{PgExecutor, Pool, Postgres, Row};
use tracing::instrument;

use crate::broadcast_utils::gas_estimation::FeesEstimate;
use crate::config::DatabaseConfig;
use crate::db::data::{NetworkInfo, RelayerInfo};
use crate::db::wrappers::h256::H256Wrapper;
use crate::db::wrappers::hex_u256::HexU256Wrapper;
use crate::types::{RelayerUpdateRequest, TransactionPriority, TxStatus};

pub mod data;
pub mod wrappers;

use self::data::{
    BlockFees, NetworkStats, ReadTxData, RelayerTxsAtMaxEscalations, RpcKind,
};
pub use self::data::{TxForEscalation, UnsentTx};

// Statically link in migration files.
//
// Migrations are expand-only and the migrator ignores applied migrations it
// doesn't know, so the previous image still starts after a newer one migrated
// the database. The migrator holds a Postgres advisory lock while it runs, so
// processes that start at the same time apply each migration once.
static MIGRATOR: Migrator = Migrator {
    ignore_missing: true,
    ..sqlx::migrate!("db/migrations")
};

pub struct Database {
    pub pool: Pool<Postgres>,
    /// Reserved for [`Database::ping`]
    probe_pool: Pool<Postgres>,
}

/// The outcome of [`Database::create_transaction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateResult {
    Created,
    /// A transaction with the same id exists
    Conflict,
    /// The relayer already has `max_queued_txs` unsent or pending
    /// transactions
    QueueFull,
}

impl Database {
    pub async fn new(config: &DatabaseConfig) -> eyre::Result<Self> {
        let connection_string = config.to_connection_string();

        if !Postgres::database_exists(&connection_string).await? {
            tracing::info!("Creating database");
            Postgres::create_database(&connection_string).await?;
        }

        tracing::info!("Connecting to the database");
        let pool = Pool::connect(&connection_string).await?;

        tracing::info!("Running migrations");
        MIGRATOR.run(&pool).await?;

        let probe_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&connection_string)?;

        tracing::info!("Database initialized");
        Ok(Self { pool, probe_pool })
    }

    /// Checks that the database answers a trivial query within `timeout`.
    ///
    /// The query runs on a connection reserved for this check, so it still
    /// answers while every other connection is busy.
    pub async fn ping(&self, timeout: Duration) -> eyre::Result<()> {
        tokio::time::timeout(
            timeout,
            sqlx::query("SELECT 1").execute(&self.probe_pool),
        )
        .await
        .map_err(|_| eyre::eyre!("No answer within {timeout:?}"))??;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn create_relayer(
        &self,
        id: &str,
        name: &str,
        chain_id: u64,
        key_id: &str,
        address: Address,
    ) -> eyre::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO relayers (id, name, chain_id, key_id, address)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT DO NOTHING
        "#,
        )
        .bind(id)
        .bind(name)
        .bind(chain_id as i64)
        .bind(key_id)
        .bind(address.as_slice())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn update_relayer(
        &self,
        id: &str,
        update: &RelayerUpdateRequest,
    ) -> eyre::Result<()> {
        let mut tx = self.pool.begin().await?;

        let RelayerUpdateRequest {
            relayer_name,
            max_inflight_txs,
            max_queued_txs,
            gas_price_limits,
            enabled,
        } = update;

        if let Some(name) = relayer_name {
            sqlx::query(
                r#"
                UPDATE relayers
                SET    name = $2
                WHERE  id = $1
                "#,
            )
            .bind(id)
            .bind(name)
            .execute(tx.as_mut())
            .await?;
        }

        if let Some(max_inflight_txs) = max_inflight_txs {
            sqlx::query(
                r#"
                UPDATE relayers
                SET    max_inflight_txs = $2
                WHERE  id = $1
                "#,
            )
            .bind(id)
            .bind(*max_inflight_txs as i64)
            .execute(tx.as_mut())
            .await?;
        }

        if let Some(max_queued_txs) = max_queued_txs {
            sqlx::query(
                r#"
                UPDATE relayers
                SET    max_queued_txs = $2
                WHERE  id = $1
                "#,
            )
            .bind(id)
            .bind(*max_queued_txs as i64)
            .execute(tx.as_mut())
            .await?;
        }

        if let Some(gas_price_limits) = gas_price_limits {
            sqlx::query(
                r#"
                UPDATE relayers
                SET    gas_price_limits = $2
                WHERE  id = $1
                "#,
            )
            .bind(id)
            .bind(Json(gas_price_limits))
            .execute(tx.as_mut())
            .await?;
        }

        if let Some(enabled) = enabled {
            sqlx::query(
                r#"
                UPDATE relayers
                SET    enabled = $2
                WHERE  id = $1
                "#,
            )
            .bind(id)
            .bind(*enabled)
            .execute(tx.as_mut())
            .await?;
        }

        tx.commit().await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn get_relayers(&self) -> eyre::Result<Vec<RelayerInfo>> {
        Ok(sqlx::query_as(
            r#"
            SELECT
                id,
                name,
                chain_id,
                key_id,
                address,
                nonce,
                current_nonce,
                max_inflight_txs,
                max_queued_txs,
                gas_price_limits,
                enabled
            FROM relayers
            "#,
        )
        .fetch_all(&self.pool)
        .await?)
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn get_relayers_by_chain_id(
        &self,
        chain_id: u64,
    ) -> eyre::Result<Vec<RelayerInfo>> {
        Ok(sqlx::query_as(
            r#"
            SELECT
                id,
                name,
                chain_id,
                key_id,
                address,
                nonce,
                current_nonce,
                max_inflight_txs,
                max_queued_txs,
                gas_price_limits,
                enabled
            FROM relayers
            WHERE chain_id = $1
            "#,
        )
        .bind(chain_id as i64)
        .fetch_all(&self.pool)
        .await?)
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn get_relayer(
        &self,
        id: &str,
    ) -> eyre::Result<Option<RelayerInfo>> {
        Ok(sqlx::query_as(
            r#"
            SELECT
                id,
                name,
                chain_id,
                key_id,
                address,
                nonce,
                current_nonce,
                max_inflight_txs,
                max_queued_txs,
                gas_price_limits,
                enabled
            FROM relayers
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Counts the unsent and pending transactions of `relayer_id`.
    #[instrument(skip(self), level = "debug")]
    pub async fn get_relayer_pending_txs(
        &self,
        relayer_id: &str,
    ) -> eyre::Result<usize> {
        count_queued_txs(&self.pool, relayer_id).await
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn get_relayer_pending_txs_gas_limit_sum(
        &self,
        relayer_id: &str,
    ) -> eyre::Result<U256> {
        let gas_limits: Vec<(HexU256Wrapper,)> = sqlx::query_as(
            r#"
            SELECT t.gas_limit
            FROM transactions t
            LEFT JOIN sent_transactions s ON (t.id = s.tx_id)
            WHERE t.relayer_id = $1
            AND (s.tx_id IS NULL OR s.status = $2)
            "#,
        )
        .bind(relayer_id)
        .bind(TxStatus::Pending)
        .fetch_all(&self.pool)
        .await?;

        Ok(gas_limits
            .into_iter()
            .fold(U256::ZERO, |acc, (v,)| acc.saturating_add(v.0)))
    }

    /// Creates a transaction of `relayer_id` with the relayer's next nonce,
    /// unless the relayer's queue is full or `tx_id` exists.
    #[instrument(skip(self), level = "debug")]
    pub async fn create_transaction(
        &self,
        tx_id: &str,
        to: Address,
        data: &[u8],
        value: U256,
        gas_limit: U256,
        priority: TransactionPriority,
        blobs: Option<Vec<Vec<u8>>>,
        relayer_id: &str,
    ) -> eyre::Result<CreateResult> {
        let mut tx = self.pool.begin().await?;

        let value_bytes: [u8; 32] = value.to_be_bytes();
        let gas_limit_bytes: [u8; 32] = gas_limit.to_be_bytes();

        // Locks the relayer, so that concurrent creates count each other's
        // transactions
        let (max_queued_txs,): (i64,) = sqlx::query_as(
            r#"
            SELECT max_queued_txs
            FROM   relayers
            WHERE  id = $1
            FOR NO KEY UPDATE
            "#,
        )
        .bind(relayer_id)
        .fetch_one(tx.as_mut())
        .await?;

        let queued_txs = count_queued_txs(tx.as_mut(), relayer_id).await?;

        if queued_txs >= max_queued_txs as usize {
            tx.rollback().await?;
            return Ok(CreateResult::QueueFull);
        }

        let (nonce,): (i64,) = sqlx::query_as(
            r#"
            UPDATE relayers
            SET nonce = nonce + 1,
                updated_at = now()
            WHERE id = $1
            RETURNING nonce - 1
            "#,
        )
        .bind(relayer_id)
        .fetch_one(tx.as_mut())
        .await?;

        let res = sqlx::query(
            r#"
            INSERT INTO transactions (id, tx_to, data, value, gas_limit, priority, relayer_id, nonce, blobs)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
        )
        .bind(tx_id)
        .bind(to.as_slice())
        .bind(data)
        .bind(value_bytes)
        .bind(gas_limit_bytes)
        .bind(priority)
        .bind(relayer_id)
        .bind(nonce)
        .bind(blobs)
        .execute(tx.as_mut())
        .await;

        if let Err(sqlx::Error::Database(ref err)) = res {
            if err.constraint() == Some("transactions_pkey") {
                return Ok(CreateResult::Conflict);
            }
        }

        res?;

        tx.commit().await?;

        Ok(CreateResult::Created)
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn get_unsent_txs(&self) -> eyre::Result<Vec<UnsentTx>> {
        Ok(sqlx::query_as(
            r#"
            SELECT     r.id as relayer_id, t.id, t.tx_to, t.data, t.value, t.gas_limit, t.priority, t.nonce, t.blobs, r.key_id, r.chain_id
            FROM       transactions t
            LEFT JOIN  sent_transactions s ON (t.id = s.tx_id)
            INNER JOIN relayers r ON (t.relayer_id = r.id)
            WHERE      s.tx_id IS NULL
            AND        (t.nonce - r.current_nonce < r.max_inflight_txs)
            ORDER BY   r.id, t.nonce ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Records the first signed version of `tx_id`, which must be stored
    /// before it is sent.
    ///
    /// Fails if `tx_id` was already broadcast, so that two broadcasts of the
    /// same transaction can't both be sent.
    #[instrument(skip(self, raw_signed_tx), level = "debug")]
    pub async fn insert_tx_broadcast(
        &self,
        tx_id: &str,
        tx_hash: B256,
        raw_signed_tx: &[u8],
        initial_max_fee_per_gas: U256,
        initial_max_priority_fee_per_gas: U256,
    ) -> eyre::Result<()> {
        let initial_max_fee_per_gas_bytes: [u8; 32] =
            initial_max_fee_per_gas.to_be_bytes();
        let initial_max_priority_fee_per_gas_bytes: [u8; 32] =
            initial_max_priority_fee_per_gas.to_be_bytes();

        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO tx_hashes (tx_id, tx_hash, raw_signed_tx, max_fee_per_gas, max_priority_fee_per_gas)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(tx_id)
        .bind(tx_hash.as_slice())
        .bind(raw_signed_tx)
        .bind(initial_max_fee_per_gas_bytes)
        .bind(initial_max_priority_fee_per_gas_bytes)
        .execute(tx.as_mut())
        .await?;

        sqlx::query(
            r#"
            INSERT INTO sent_transactions (tx_id, initial_max_fee_per_gas, initial_max_priority_fee_per_gas, valid_tx_hash)
            VALUES ($1, $2, $3, $4)
            "#
        )
        .bind(tx_id)
        .bind(initial_max_fee_per_gas_bytes)
        .bind(initial_max_priority_fee_per_gas_bytes)
        .bind(tx_hash.as_slice())
        .execute(tx.as_mut()).await?;

        tx.commit().await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn get_latest_block_number_without_fee_estimates(
        &self,
        chain_id: u64,
    ) -> eyre::Result<Option<u64>> {
        let block_number: Option<(i64,)> = sqlx::query_as(
            r#"
            SELECT block_number
            FROM   blocks
            WHERE  chain_id = $1
            AND    block_number NOT IN (
                SELECT block_number
                FROM   block_fees
                WHERE  chain_id = $1
            )
            ORDER BY block_number DESC
            LIMIT  1
            "#,
        )
        .bind(chain_id as i64)
        .fetch_optional(&self.pool)
        .await?;

        Ok(block_number.map(|(n,)| n as u64))
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn get_latest_block_number(
        &self,
        chain_id: u64,
    ) -> eyre::Result<Option<u64>> {
        let block_number: Option<(i64,)> = sqlx::query_as(
            r#"
            SELECT block_number
            FROM   blocks
            WHERE  chain_id = $1
            ORDER BY block_number DESC
            LIMIT  1
            "#,
        )
        .bind(chain_id as i64)
        .fetch_optional(&self.pool)
        .await?;

        Ok(block_number.map(|(n,)| n as u64))
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn get_latest_block_fees_by_chain_id(
        &self,
        chain_id: u64,
    ) -> eyre::Result<Option<BlockFees>> {
        let row: Option<(Json<FeesEstimate>, BigDecimal)> = sqlx::query_as(
            r#"
            SELECT   bf.fee_estimate, bf.gas_price
            FROM     blocks b
            JOIN     block_fees bf ON (b.block_number = bf.block_number AND b.chain_id = bf.chain_id)
            WHERE    b.chain_id = $1
            ORDER BY b.block_number DESC
            LIMIT    1
            "#,
        )
        .bind(chain_id as i64)
        .fetch_optional(&self.pool)
        .await?;

        let Some((fees, gas_price)) = row else {
            return Ok(None);
        };

        let fee_estimates = fees.0;

        let gas_price_str = gas_price.to_string();
        let gas_price = U256::from_str_radix(&gas_price_str, 10)?;

        Ok(Some(BlockFees {
            fee_estimates,
            gas_price,
        }))
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn has_blocks_for_chain(
        &self,
        chain_id: u64,
    ) -> eyre::Result<bool> {
        let row = sqlx::query(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM blocks
                WHERE chain_id = $1
            )
            "#,
        )
        .bind(chain_id as i64)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.try_get::<bool, _>(0)?)
    }

    /// Saves block `block_number` of `chain_id` with those of `txs` that are
    /// hashes of our transactions, replacing an earlier version of the block.
    ///
    /// Saving the same block again, for example from two indexers, keeps its
    /// fee estimates.
    #[instrument(skip(self), level = "debug")]
    pub async fn save_block(
        &self,
        block_number: u64,
        chain_id: u64,
        timestamp: DateTime<Utc>,
        txs: &[B256],
    ) -> eyre::Result<()> {
        let mut db_tx = self.pool.begin().await?;

        // Locks the block, so concurrent saves replace its txs one at a time
        sqlx::query(
            r#"
            INSERT INTO blocks (block_number, chain_id, timestamp)
            VALUES ($1, $2, $3)
            ON CONFLICT (block_number, chain_id) DO UPDATE
            SET timestamp = EXCLUDED.timestamp
            "#,
        )
        .bind(block_number as i64)
        .bind(chain_id as i64)
        .bind(timestamp)
        .execute(db_tx.as_mut())
        .await?;

        sqlx::query(
            r#"
            DELETE
            FROM    block_txs
            WHERE   block_number = $1
            AND     chain_id = $2
            "#,
        )
        .bind(block_number as i64)
        .bind(chain_id as i64)
        .execute(db_tx.as_mut())
        .await?;

        let txs: Vec<_> = txs.iter().map(|tx| H256Wrapper(*tx)).collect();

        sqlx::query(
            r#"
            INSERT INTO block_txs (block_number, chain_id, tx_hash)
            SELECT $1, $2, unnested.tx_hash
            FROM UNNEST($3::BYTEA[]) AS unnested(tx_hash)
            WHERE EXISTS (
                SELECT 1
                FROM tx_hashes
                WHERE tx_hashes.tx_hash = unnested.tx_hash
            )
            "#,
        )
        .bind(block_number as i64)
        .bind(chain_id as i64)
        .bind(&txs[..])
        .execute(db_tx.as_mut())
        .await?;

        db_tx.commit().await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn save_block_fees(
        &self,
        block_number: u64,
        chain_id: u64,
        fee_estimates: &FeesEstimate,
        gas_price: U256,
    ) -> eyre::Result<()> {
        // TODO: Figure out how to do this without parsing
        let gas_price: BigDecimal = gas_price.to_string().parse()?;

        sqlx::query(
            r#"
            INSERT INTO block_fees (block_number, chain_id, fee_estimate, gas_price)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (block_number, chain_id) DO UPDATE
            SET fee_estimate = EXCLUDED.fee_estimate,
                gas_price = EXCLUDED.gas_price
            "#,
        )
        .bind(block_number as i64)
        .bind(chain_id as i64)
        .bind(Json(fee_estimates))
        .bind(gas_price)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Returns a list of soft reorged txs
    #[instrument(skip(self), level = "debug", ret)]
    pub async fn handle_soft_reorgs(&self) -> eyre::Result<Vec<String>> {
        let mut tx = self.pool.begin().await?;

        // Fetch txs which have valid tx hash different than what is actually mined
        let items: Vec<(String, H256Wrapper)> = sqlx::query_as(
            r#"
            SELECT t.id, h.tx_hash
            FROM  transactions t
            JOIN  sent_transactions s ON t.id = s.tx_id
            JOIN  tx_hashes h ON t.id = h.tx_id
            JOIN  block_txs bt ON h.tx_hash = bt.tx_hash
            WHERE h.tx_hash <> s.valid_tx_hash
            AND   s.status = $1
            "#,
        )
        .bind(TxStatus::Mined)
        .fetch_all(tx.as_mut())
        .await?;

        let (tx_ids, tx_hashes): (Vec<_>, Vec<_>) = items.into_iter().unzip();

        sqlx::query(
            r#"
            UPDATE sent_transactions s
            SET    valid_tx_hash = mined.tx_hash
            FROM   transactions t,
                   UNNEST($1::TEXT[], $2::BYTEA[]) AS mined(tx_id, tx_hash)
            WHERE  t.id = mined.tx_id
            AND    t.id = s.tx_id
            AND    s.status = $3
            "#,
        )
        .bind(&tx_ids)
        .bind(&tx_hashes)
        .bind(TxStatus::Mined)
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;

        Ok(tx_ids)
    }

    /// Returns a list of hard reorged txs
    #[instrument(skip(self), level = "debug", ret)]
    pub async fn handle_hard_reorgs(&self) -> eyre::Result<Vec<String>> {
        let mut tx = self.pool.begin().await?;

        // Fetch txs which are marked as mined
        // but none of the associated tx hashes are present in block txs
        let items: Vec<(String,)> = sqlx::query_as(
            r#"
            WITH reorg_candidates AS (
                SELECT     t.id, h.tx_hash, bt.chain_id
                FROM       transactions t
                JOIN       sent_transactions s ON t.id = s.tx_id
                JOIN       tx_hashes h ON t.id = h.tx_id
                LEFT JOIN  block_txs bt ON h.tx_hash = bt.tx_hash
                WHERE      s.status = $1
            )
            SELECT    r.id
            FROM      reorg_candidates r
            GROUP BY  r.id
            HAVING    COUNT(r.chain_id) = 0
            "#,
        )
        .bind(TxStatus::Mined)
        .fetch_all(tx.as_mut())
        .await?;

        let tx_ids: Vec<_> = items.into_iter().map(|(x,)| x).collect();

        // Set status to pending
        // and set valid tx hash to the latest tx hash.
        // The status may have changed since the select, so check it again
        sqlx::query(
            r#"
            UPDATE sent_transactions s
            SET    status = $1,
                   valid_tx_hash = (
                       SELECT tx_hash
                       FROM   tx_hashes h
                       WHERE  h.tx_id = s.tx_id
                       ORDER BY created_at DESC
                       LIMIT  1
                   ),
                   mined_at = NULL
            FROM   transactions t, UNNEST($2::TEXT[]) AS reorged(tx_id)
            WHERE  t.id = reorged.tx_id
            AND    t.id = s.tx_id
            AND    s.status = $3
            "#,
        )
        .bind(TxStatus::Pending)
        .bind(&tx_ids)
        .bind(TxStatus::Mined)
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;

        Ok(tx_ids)
    }

    /// Marks txs as mined if the associated tx hash is present in a block
    ///
    /// returns the tx ids and hashes for all mined txs
    #[instrument(skip(self), level = "debug", ret)]
    pub async fn mine_txs(
        &self,
        chain_id: u64,
    ) -> eyre::Result<Vec<(String, B256)>> {
        let updated_txs: Vec<(String, H256Wrapper)> = sqlx::query_as(
            r#"
            WITH cte AS (
                SELECT t.id, h.tx_hash, b.timestamp
                FROM   transactions t
                JOIN   sent_transactions s ON t.id = s.tx_id
                JOIN   tx_hashes h ON t.id = h.tx_id
                JOIN   block_txs bt ON h.tx_hash = bt.tx_hash
                JOIN   blocks b ON
                           bt.block_number = b.block_number
                       AND bt.chain_id = b.chain_id
                WHERE  s.status = $1
                AND    b.chain_id = $2
            )
            UPDATE    sent_transactions
            SET       status = $3,
                      valid_tx_hash = cte.tx_hash,
                      mined_at = cte.timestamp
            FROM      cte
            WHERE     sent_transactions.tx_id = cte.id
            AND       sent_transactions.status = $1
            RETURNING sent_transactions.tx_id, sent_transactions.valid_tx_hash
            "#,
        )
        .bind(TxStatus::Pending)
        .bind(chain_id as i64)
        .bind(TxStatus::Mined)
        .fetch_all(&self.pool)
        .await?;

        Ok(updated_txs
            .into_iter()
            .map(|(id, hash)| (id, hash.0))
            .collect())
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn finalize_txs(
        &self,
        finalization_timestmap: DateTime<Utc>,
    ) -> eyre::Result<()> {
        let mut tx = self.pool.begin().await?;

        // Fetch txs which are marked as mined, but the associated valid tx hash
        // is present in a block which is older than the given timestamp
        let items: Vec<(String,)> = sqlx::query_as(
            r#"
            SELECT s.tx_id
            FROM   sent_transactions s
            JOIN   tx_hashes h ON s.valid_tx_hash = h.tx_hash
            JOIN   block_txs bt ON h.tx_hash = bt.tx_hash
            JOIN   blocks b ON bt.block_number = b.block_number AND bt.chain_id = b.chain_id
            WHERE  s.status = $1
            AND    b.timestamp < $2
            "#,
        )
        .bind(TxStatus::Mined)
        .bind(finalization_timestmap)
        .fetch_all(tx.as_mut())
        .await?;

        let tx_ids: Vec<_> = items.into_iter().map(|(x,)| x).collect();

        // Set status to finalized.
        // The status may have changed since the select, so check it again
        sqlx::query(
            r#"
            UPDATE sent_transactions s
            SET    status = $1
            FROM   transactions t, UNNEST($2::TEXT[]) AS finalized(tx_id)
            WHERE  t.id = finalized.tx_id
            AND    t.id = s.tx_id
            AND    s.status = $3
            "#,
        )
        .bind(TxStatus::Finalized)
        .bind(&tx_ids)
        .bind(TxStatus::Mined)
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;

        Ok(())
    }

    /// Returns the pending transactions whose latest signed version was
    /// stored more than `escalation_interval` ago, however often they were
    /// escalated.
    #[instrument(skip(self), level = "debug")]
    pub async fn get_txs_for_escalation(
        &self,
        escalation_interval: Duration,
    ) -> eyre::Result<Vec<TxForEscalation>> {
        Ok(sqlx::query_as(
            r#"
            SELECT r.id as relayer_id, t.id, t.tx_to, t.data, t.value, t.gas_limit, t.nonce,
                   t.blobs, r.key_id, r.chain_id,
                   s.initial_max_fee_per_gas, s.initial_max_priority_fee_per_gas, s.escalation_count,
                   h.raw_signed_tx,
                   GREATEST(EXTRACT(EPOCH FROM now() - h.created_at) * 1000, 0)::BIGINT AS latest_hash_age_ms
            FROM   transactions t
            JOIN   sent_transactions s ON t.id = s.tx_id
            JOIN   tx_hashes h ON t.id = h.tx_id
            JOIN   relayers r ON t.relayer_id = r.id
            WHERE  now() - h.created_at > $1
            AND    s.status = $2
            AND    NOT h.escalated
            "#,
        )
        .bind(escalation_interval)
        .bind(TxStatus::Pending)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Counts the pending transactions of every relayer that have at least
    /// `max_escalations` escalations, including relayers without any.
    #[instrument(skip(self), level = "debug")]
    pub async fn count_txs_at_max_escalations(
        &self,
        max_escalations: usize,
    ) -> eyre::Result<Vec<RelayerTxsAtMaxEscalations>> {
        Ok(sqlx::query_as(
            r#"
            SELECT    r.id AS relayer_id, r.chain_id, COUNT(t.id) AS txs
            FROM      relayers r
            LEFT JOIN (
                          transactions t
                          JOIN sent_transactions s ON (
                              s.tx_id = t.id
                          AND s.status = $1
                          AND s.escalation_count >= $2
                          )
                      ) ON t.relayer_id = r.id
            GROUP BY  r.id
            "#,
        )
        .bind(TxStatus::Pending)
        .bind(max_escalations as i64)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Records `tx_hash` as the replacement of `tx_id` that follows its
    /// `escalation_count` earlier escalations. It must be stored before it is
    /// sent. The versions it replaces keep their hashes, so that indexing
    /// still finds them mined, but drop their signed bytes, which are never
    /// sent again.
    ///
    /// Returns `false`, and changes nothing, unless `tx_id` is still pending
    /// with exactly `escalation_count` escalations. Then another escalation or
    /// a status change won the race, and this replacement must not be sent.
    #[instrument(skip(self, raw_signed_tx), level = "debug")]
    pub async fn escalate_tx(
        &self,
        tx_id: &str,
        escalation_count: usize,
        tx_hash: B256,
        raw_signed_tx: &[u8],
        max_fee_per_gas: U256,
        max_priority_fee_per_gas: U256,
    ) -> eyre::Result<bool> {
        let mut tx = self.pool.begin().await?;

        // Locks the row, so a concurrent escalation waits here and then finds
        // the count changed
        let escalated = sqlx::query(
            r#"
            UPDATE sent_transactions
            SET    escalation_count = escalation_count + 1,
                   updated_at = now()
            WHERE  tx_id = $1
            AND    escalation_count = $2
            AND    status = $3
            "#,
        )
        .bind(tx_id)
        .bind(escalation_count as i64)
        .bind(TxStatus::Pending)
        .execute(tx.as_mut())
        .await?;

        if escalated.rows_affected() == 0 {
            tx.rollback().await?;
            return Ok(false);
        }

        sqlx::query(
            r#"
            UPDATE tx_hashes
            SET    escalated = true,
                   raw_signed_tx = NULL
            WHERE  tx_id = $1
            AND    NOT escalated
            "#,
        )
        .bind(tx_id)
        .execute(tx.as_mut())
        .await?;

        let max_fee_per_gas_bytes: [u8; 32] = max_fee_per_gas.to_be_bytes();
        let max_priority_fee_per_gas_bytes: [u8; 32] =
            max_priority_fee_per_gas.to_be_bytes();

        sqlx::query(
            r#"
            INSERT INTO tx_hashes (tx_id, tx_hash, raw_signed_tx, max_fee_per_gas, max_priority_fee_per_gas)
            VALUES ($1, $2, $3, $4, $5)
            "#
        )
        .bind(tx_id)
        .bind(tx_hash.as_slice())
        .bind(raw_signed_tx)
        .bind(max_fee_per_gas_bytes)
        .bind(max_priority_fee_per_gas_bytes)
        .execute(tx.as_mut())
        .await?;

        sqlx::query(
            r#"
            UPDATE sent_transactions
            SET    valid_tx_hash = $2
            WHERE  tx_id = $1
            "#,
        )
        .bind(tx_id)
        .bind(tx_hash.as_slice())
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;

        Ok(true)
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn read_tx(
        &self,
        tx_id: &str,
    ) -> eyre::Result<Option<ReadTxData>> {
        Ok(sqlx::query_as(
            r#"
            SELECT t.id as tx_id, t.tx_to as to, t.data, t.value, t.gas_limit, t.nonce,
                   t.blobs, h.tx_hash, s.status
            FROM transactions t
            LEFT JOIN sent_transactions s ON t.id = s.tx_id
            LEFT JOIN tx_hashes h ON s.valid_tx_hash = h.tx_hash
            WHERE t.id = $1
            "#,
        )
        .bind(tx_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn read_relayer_tx(
        &self,
        relayer_id: &str,
        tx_id: &str,
    ) -> eyre::Result<Option<ReadTxData>> {
        Ok(sqlx::query_as(
            r#"
            SELECT t.id as tx_id, t.tx_to as to, t.data, t.value, t.gas_limit, t.nonce,
                   t.blobs, h.tx_hash, s.status
            FROM transactions t
            LEFT JOIN sent_transactions s ON t.id = s.tx_id
            LEFT JOIN tx_hashes h ON s.valid_tx_hash = h.tx_hash
            WHERE t.id = $1
            AND   t.relayer_id = $2
            "#,
        )
        .bind(tx_id)
        .bind(relayer_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn read_relayer_txs(
        &self,
        relayer_id: &str,
        tx_status_filter: Option<Option<TxStatus>>,
    ) -> eyre::Result<Vec<ReadTxData>> {
        let (should_filter, status_filter) = match tx_status_filter {
            Some(status) => (true, status),
            None => (false, None),
        };

        Ok(sqlx::query_as(
            r#"
            SELECT t.id as tx_id, t.tx_to as to, t.data, t.value, t.gas_limit, t.nonce,
                   t. blobs, h.tx_hash, s.status
            FROM transactions t
            LEFT JOIN sent_transactions s ON t.id = s.tx_id
            LEFT JOIN tx_hashes h ON s.valid_tx_hash = h.tx_hash
            WHERE t.relayer_id = $1
            AND   (($2 = true AND s.status = $3) OR $2 = false)
            "#,
        )
        .bind(relayer_id)
        .bind(should_filter)
        .bind(status_filter)
        .fetch_all(&self.pool)
        .await?)
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn read_txs(
        &self,
        tx_status_filter: Option<Option<TxStatus>>,
    ) -> eyre::Result<Vec<ReadTxData>> {
        let (should_filter, status_filter) = match tx_status_filter {
            Some(status) => (true, status),
            None => (false, None),
        };

        Ok(sqlx::query_as(
            r#"
            SELECT t.id as tx_id, t.tx_to as to, t.data, t.value, t.gas_limit, t.nonce,
                   t. blobs, h.tx_hash, s.status
            FROM transactions t
            LEFT JOIN sent_transactions s ON t.id = s.tx_id
            LEFT JOIN tx_hashes h ON s.valid_tx_hash = h.tx_hash
            WHERE ($1 = true AND s.status = $2) OR $1 = false
            "#,
        )
        .bind(should_filter)
        .bind(status_filter)
        .fetch_all(&self.pool)
        .await?)
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn update_relayer_nonce(
        &self,
        chain_id: u64,
        relayer_address: Address,
        nonce: u64,
    ) -> eyre::Result<()> {
        sqlx::query(
            r#"
            UPDATE relayers
            SET    current_nonce = $3,
                   updated_at = now()
            WHERE  chain_id = $1
            AND    address = $2
            "#,
        )
        .bind(chain_id as i64)
        .bind(relayer_address.as_slice())
        .bind(nonce as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn prune_blocks(
        &self,
        timestamp: DateTime<Utc>,
    ) -> eyre::Result<()> {
        sqlx::query(
            r#"
            DELETE FROM blocks
            WHERE  timestamp < $1
            "#,
        )
        .bind(timestamp)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn prune_txs(
        &self,
        timestamp: DateTime<Utc>,
    ) -> eyre::Result<()> {
        sqlx::query(
            r#"
            DELETE
            FROM  transactions t
            USING sent_transactions s
            WHERE t.id = s.tx_id
            AND   s.mined_at < $1
            AND   s.status = $2
            "#,
        )
        .bind(timestamp)
        .bind(TxStatus::Finalized)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn upsert_network(
        &self,
        chain_id: u64,
        name: &str,
        http_rpc: &str,
        ws_rpc: &str,
    ) -> eyre::Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO networks (chain_id, name)
            VALUES ($1, $2)
            ON CONFLICT (chain_id) DO UPDATE
            SET name = EXCLUDED.name
            "#,
        )
        .bind(chain_id as i64)
        .bind(name)
        .execute(tx.as_mut())
        .await?;

        sqlx::query(
            r#"
            INSERT INTO rpcs (chain_id, url, kind)
            VALUES
                ($1, $2, $3)
            ON CONFLICT (chain_id, kind) DO UPDATE
            SET url = EXCLUDED.url
            "#,
        )
        .bind(chain_id as i64)
        .bind(http_rpc)
        .bind(RpcKind::Http)
        .execute(tx.as_mut())
        .await?;

        sqlx::query(
            r#"
            INSERT INTO rpcs (chain_id, url, kind)
            VALUES
                ($1, $2, $3)
            ON CONFLICT (chain_id, kind) DO UPDATE
            SET url = EXCLUDED.url
            "#,
        )
        .bind(chain_id as i64)
        .bind(ws_rpc)
        .bind(RpcKind::Ws)
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn get_network_rpc(
        &self,
        chain_id: u64,
        rpc_kind: RpcKind,
    ) -> eyre::Result<String> {
        let row: (String,) = sqlx::query_as(
            r#"
            SELECT url
            FROM   rpcs
            WHERE  chain_id = $1
            AND    kind = $2
            "#,
        )
        .bind(chain_id as i64)
        .bind(rpc_kind)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.0)
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn get_network_chain_ids(&self) -> eyre::Result<Vec<u64>> {
        let items: Vec<(i64,)> = sqlx::query_as(
            r#"
            SELECT chain_id
            FROM   networks
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(items.into_iter().map(|(x,)| x as u64).collect())
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn get_networks(&self) -> eyre::Result<Vec<NetworkInfo>> {
        Ok(sqlx::query_as(
            r#"
            SELECT networks.chain_id, name, http.url as http_rpc, ws.url as ws_rpc
            FROM   networks
            INNER JOIN rpcs http ON networks.chain_id = http.chain_id AND http.kind = 'http'
            INNER JOIN rpcs ws ON networks.chain_id = ws.chain_id AND ws.kind = 'ws'
            "#,
        )
        .fetch_all(&self.pool)
        .await?)
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn get_network(
        &self,
        chain_id: u64,
    ) -> eyre::Result<Option<NetworkInfo>> {
        Ok(sqlx::query_as(
            r#"
            SELECT networks.chain_id, name, http.url as http_rpc, ws.url as ws_rpc
            FROM   networks
            INNER JOIN rpcs http ON networks.chain_id = http.chain_id AND http.kind = 'http'
            INNER JOIN rpcs ws ON networks.chain_id = ws.chain_id AND ws.kind = 'ws'
            WHERE networks.chain_id = $1
            "#,
        )
        .bind(chain_id as i64)
        .fetch_optional(&self.pool)
        .await?)
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn upsert_api_key(
        &self,
        relayer_id: &str,
        api_key_hash: [u8; 32],
    ) -> eyre::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO api_keys (relayer_id, key_hash)
            VALUES ($1, $2)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(relayer_id)
        .bind(api_key_hash)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn is_api_key_valid(
        &self,
        relayer_id: &str,
        api_key_hash: [u8; 32],
    ) -> eyre::Result<bool> {
        let (is_valid,): (bool,) = sqlx::query_as(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM api_keys
                WHERE relayer_id = $1
                AND   key_hash = $2
            )
            "#,
        )
        .bind(relayer_id)
        .bind(api_key_hash)
        .fetch_one(&self.pool)
        .await?;

        Ok(is_valid)
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn get_stats(&self, chain_id: u64) -> eyre::Result<NetworkStats> {
        let (pending_txs,): (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(1)
            FROM transactions t
            JOIN relayers r ON (t.relayer_id = r.id)
            LEFT JOIN sent_transactions s ON (t.id = s.tx_id)
            WHERE s.tx_id IS NULL
            AND r.chain_id = $1
            "#,
        )
        .bind(chain_id as i64)
        .fetch_one(&self.pool)
        .await?;

        let (mined_txs,): (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(1)
            FROM transactions t
            JOIN relayers r ON (t.relayer_id = r.id)
            LEFT JOIN sent_transactions s ON (t.id = s.tx_id)
            WHERE s.status = $1
            AND   r.chain_id = $2
            "#,
        )
        .bind(TxStatus::Mined)
        .bind(chain_id as i64)
        .fetch_one(&self.pool)
        .await?;

        let (finalized_txs,): (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(1)
            FROM transactions t
            JOIN relayers r ON (t.relayer_id = r.id)
            LEFT JOIN sent_transactions s ON (t.id = s.tx_id)
            WHERE s.status = $1
            AND   r.chain_id = $2
            "#,
        )
        .bind(TxStatus::Finalized)
        .bind(chain_id as i64)
        .fetch_one(&self.pool)
        .await?;

        let (total_indexed_blocks,): (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(1)
            FROM blocks
            WHERE chain_id = $1
            "#,
        )
        .bind(chain_id as i64)
        .fetch_one(&self.pool)
        .await?;

        let (block_txs,): (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(1)
            FROM block_txs
            WHERE chain_id = $1
            "#,
        )
        .bind(chain_id as i64)
        .fetch_one(&self.pool)
        .await?;

        Ok(NetworkStats {
            pending_txs: pending_txs as u64,
            mined_txs: mined_txs as u64,
            finalized_txs: finalized_txs as u64,
            total_indexed_blocks: total_indexed_blocks as u64,
            block_txs: block_txs as u64,
        })
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn purge_unsent_txs(&self, relayer_id: &str) -> eyre::Result<()> {
        sqlx::query(
            r#"
            UPDATE relayers
            SET nonce = current_nonce
            WHERE id = $1
            "#,
        )
        .bind(relayer_id)
        .execute(&self.pool)
        .await?;

        sqlx::query(
            r#"
            DELETE FROM transactions
            WHERE relayer_id = $1
            AND id NOT IN (
                SELECT tx_id FROM sent_transactions
            )
            "#,
        )
        .bind(relayer_id)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

async fn count_queued_txs(
    executor: impl PgExecutor<'_>,
    relayer_id: &str,
) -> eyre::Result<usize> {
    let (tx_count,): (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(1)
        FROM transactions t
        LEFT JOIN sent_transactions s ON (t.id = s.tx_id)
        WHERE t.relayer_id = $1
        AND (s.tx_id IS NULL OR s.status = $2)
        "#,
    )
    .bind(relayer_id)
    .bind(TxStatus::Pending)
    .fetch_one(executor)
    .await?;

    Ok(tx_count as usize)
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use eyre::ContextCompat;
    use hex_literal::hex;
    use postgres_docker_utils::DockerContainerGuard;
    use serde_json::json;

    use super::*;
    use crate::db::data::RelayerGasPriceLimit;
    use crate::db::wrappers::address::AddressWrapper;
    use crate::db::wrappers::decimal_u256::DecimalU256Wrapper;
    use crate::types::{RelayerGasPriceLimitResponse, RelayerUpdateRequest};

    const ADDRESS: [u8; 20] = hex!("1ed53d680b8890dae2a63f673a85ffde1fd5c7a2");
    const HASH: [u8; 32] = hex!(
        "46239dbfe5502b9f82c3dff992927d8d9b3168e732b4fd5771288569f5a1813d"
    );

    async fn setup_db() -> eyre::Result<(Database, DockerContainerGuard)> {
        let db_container = postgres_docker_utils::setup().await?;
        let db_socket_addr = db_container.address();
        let url =
            format!("postgres://postgres:postgres@{db_socket_addr}/database");

        for _ in 0..5 {
            match Database::new(&DatabaseConfig::connection_string(&url)).await
            {
                Ok(db) => return Ok((db, db_container)),
                Err(err) => {
                    eprintln!("Failed to connect to the database: {err:?}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }

        Err(eyre::eyre!("Failed to connect to the database"))
    }

    async fn full_update(
        db: &Database,
        chain_id: u64,
        finalization_timestamp: DateTime<Utc>,
    ) -> eyre::Result<()> {
        db.mine_txs(chain_id).await?;

        db.handle_soft_reorgs().await?;
        db.handle_hard_reorgs().await?;

        db.finalize_txs(finalization_timestamp).await?;

        Ok(())
    }

    #[tokio::test]
    async fn migration() -> eyre::Result<()> {
        let (_db, _db_container) = setup_db().await?;

        Ok(())
    }

    #[tokio::test]
    async fn starts_after_a_newer_image_migrated() -> eyre::Result<()> {
        let (db, db_container) = setup_db().await?;

        sqlx::query(
            r#"
            INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
            VALUES (99999, 'from a newer image', TRUE, '\x00', 0)
            "#,
        )
        .execute(&db.pool)
        .await?;

        let url = format!(
            "postgres://postgres:postgres@{}/database",
            db_container.address()
        );
        Database::new(&DatabaseConfig::connection_string(&url)).await?;

        Ok(())
    }

    #[tokio::test]
    async fn reset_relayer() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;

        let chain_id = 123;
        let network_name = "network_name";
        let http_rpc = "http_rpc";
        let ws_rpc = "ws_rpc";

        db.upsert_network(chain_id, network_name, http_rpc, ws_rpc)
            .await?;

        let relayer_id = uuid();
        let relayer_id = relayer_id.as_str();
        let relayer_name = "relayer_name";
        let key_id = "key_id";
        let address = Address::with_last_byte(1);

        db.create_relayer(relayer_id, relayer_name, chain_id, key_id, address)
            .await?;

        db.purge_unsent_txs(relayer_id).await?;

        Ok(())
    }

    #[tokio::test]
    async fn save_and_prune_blocks() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;

        let block_timestamp = NaiveDate::from_ymd_opt(2023, 11, 23)
            .unwrap()
            .and_hms_opt(12, 32, 2)
            .unwrap()
            .and_utc();

        let prune_timestamp = NaiveDate::from_ymd_opt(2023, 11, 23)
            .unwrap()
            .and_hms_opt(13, 32, 23)
            .unwrap()
            .and_utc();

        let tx_hashes = vec![
            B256::with_last_byte(1),
            B256::with_last_byte(2),
            B256::with_last_byte(3),
        ];

        db.save_block(1, 1, block_timestamp, &tx_hashes).await?;

        assert!(db.has_blocks_for_chain(1).await?, "Should have blocks");

        db.prune_blocks(prune_timestamp).await?;

        assert!(!db.has_blocks_for_chain(1).await?, "Should not have blocks");

        Ok(())
    }

    #[tokio::test]
    async fn relayer_methods() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;

        let chain_id = 123;
        let network_name = "network_name";
        let http_rpc = "http_rpc";
        let ws_rpc = "ws_rpc";

        db.upsert_network(chain_id, network_name, http_rpc, ws_rpc)
            .await?;

        let relayer_id = uuid();
        let relayer_id = relayer_id.as_str();

        let relayer_name = "relayer_name";
        let key_id = "key_id";
        let relayer_address = Address::with_last_byte(1);

        db.create_relayer(
            relayer_id,
            relayer_name,
            chain_id,
            key_id,
            relayer_address,
        )
        .await?;

        let relayer = db
            .get_relayer(relayer_id)
            .await?
            .context("Missing relayer")?;

        assert_eq!(relayer.id, relayer_id);
        assert_eq!(relayer.name, relayer_name);
        assert_eq!(relayer.chain_id, chain_id);
        assert_eq!(relayer.key_id, key_id);
        assert_eq!(relayer.address.0, relayer_address);
        assert_eq!(relayer.nonce, 0);
        assert_eq!(relayer.current_nonce, 0);
        assert_eq!(relayer.max_inflight_txs, 5);
        assert_eq!(relayer.gas_price_limits, vec![]);

        db.update_relayer(
            relayer_id,
            &RelayerUpdateRequest {
                relayer_name: None,
                max_inflight_txs: Some(10),
                max_queued_txs: Some(20),
                gas_price_limits: Some(vec![RelayerGasPriceLimitResponse {
                    chain_id: 1,
                    value: U256::from(10_123u64).into(),
                }]),
                enabled: None,
            },
        )
        .await?;

        let relayer = db
            .get_relayer(relayer_id)
            .await?
            .context("Missing relayer")?;

        assert_eq!(relayer.id, relayer_id);
        assert_eq!(relayer.name, relayer_name);
        assert_eq!(relayer.chain_id, chain_id);
        assert_eq!(relayer.key_id, key_id);
        assert_eq!(relayer.address.0, relayer_address);
        assert_eq!(relayer.nonce, 0);
        assert_eq!(relayer.current_nonce, 0);
        assert_eq!(relayer.max_inflight_txs, 10);
        assert_eq!(relayer.max_queued_txs, 20);
        assert_eq!(
            relayer.gas_price_limits,
            vec![RelayerGasPriceLimit {
                chain_id: 1,
                value: U256::from(10_123u64).into(),
            }]
        );

        Ok(())
    }

    #[tokio::test]
    async fn tx_lifecycle() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;

        let chain_id = 123;
        let network_name = "network_name";
        let http_rpc = "http_rpc";
        let ws_rpc = "ws_rpc";

        db.upsert_network(chain_id, network_name, http_rpc, ws_rpc)
            .await?;

        let relayer_id = uuid();
        let relayer_id = relayer_id.as_str();

        let relayer_name = "relayer_name";
        let key_id = "key_id";
        let relayer_address = Address::with_last_byte(1);

        db.create_relayer(
            relayer_id,
            relayer_name,
            chain_id,
            key_id,
            relayer_address,
        )
        .await?;

        let tx_id = "tx_id";
        let to = Address::with_last_byte(1);
        let data: &[u8] = &[];
        let value = U256::from(0);
        let gas_limit = U256::from(0);
        let priority = TransactionPriority::Regular;
        let blobs = None;

        let tx = db.read_tx(tx_id).await?;
        assert!(tx.is_none(), "Tx has not been sent yet");

        db.create_transaction(
            tx_id, to, data, value, gas_limit, priority, blobs, relayer_id,
        )
        .await?;

        let tx = db.read_tx(tx_id).await?.context("Missing tx")?;

        assert_eq!(tx.tx_id, tx_id);
        assert_eq!(tx.to.0, to);
        assert_eq!(tx.data, data);
        assert_eq!(tx.value.0, value);
        assert_eq!(tx.gas_limit.0, gas_limit);
        assert_eq!(tx.nonce, 0);
        assert_eq!(tx.tx_hash, None);
        assert_eq!(tx.blobs, None);

        let unsent_txs = db.read_relayer_txs(relayer_id, None).await?;
        assert_eq!(unsent_txs.len(), 1, "1 unsent tx");

        let tx_hash_1 = B256::with_last_byte(1);
        let tx_hash_2 = B256::with_last_byte(2);
        let initial_max_fee_per_gas = U256::from(1);
        let initial_max_priority_fee_per_gas = U256::from(1);

        db.insert_tx_broadcast(
            tx_id,
            tx_hash_1,
            &[1],
            initial_max_fee_per_gas,
            initial_max_priority_fee_per_gas,
        )
        .await?;

        let tx = db.read_tx(tx_id).await?.context("Missing tx")?;

        assert_eq!(tx.tx_hash.unwrap().0, tx_hash_1);
        assert_eq!(tx.status, Some(TxStatus::Pending));

        let unsent_txs = db.read_relayer_txs(relayer_id, Some(None)).await?;
        assert_eq!(unsent_txs.len(), 0, "0 unsent tx");

        let pending_txs = db
            .read_relayer_txs(relayer_id, Some(Some(TxStatus::Pending)))
            .await?;
        assert_eq!(pending_txs.len(), 1, "1 pending tx");

        let all_txs = db.read_relayer_txs(relayer_id, None).await?;

        assert_eq!(all_txs, pending_txs);

        let is_escalated = db
            .escalate_tx(
                tx_id,
                0,
                tx_hash_2,
                &[2],
                initial_max_fee_per_gas,
                initial_max_priority_fee_per_gas,
            )
            .await?;
        assert!(is_escalated);

        let tx = db.read_tx(tx_id).await?.context("Missing tx")?;

        // By default we take the latest tx
        assert_eq!(tx.tx_hash.unwrap().0, tx_hash_2);
        assert_eq!(tx.status, Some(TxStatus::Pending));

        // Do an update
        let finalized_timestamp = ymd_hms(2023, 11, 23, 10, 32, 3);
        full_update(&db, chain_id, finalized_timestamp).await?;

        let tx = db.read_tx(tx_id).await?.context("Missing tx")?;

        // Nothing should have changed
        assert_eq!(tx.tx_hash.unwrap().0, tx_hash_2);
        assert_eq!(tx.status, Some(TxStatus::Pending));

        // Save block
        let block_number = 1;
        let timestamp = ymd_hms(2023, 11, 23, 12, 32, 2);
        let txs = &[tx_hash_1];

        db.save_block(block_number, chain_id, timestamp, txs)
            .await?;

        full_update(&db, chain_id, finalized_timestamp).await?;

        let tx = db.read_tx(tx_id).await?.context("Missing tx")?;

        assert_eq!(tx.tx_hash.unwrap().0, tx_hash_1);
        assert_eq!(tx.status, Some(TxStatus::Mined));

        // Reorg
        let txs = &[tx_hash_2];

        db.save_block(block_number, chain_id, timestamp, txs)
            .await?;

        full_update(&db, chain_id, finalized_timestamp).await?;

        let tx = db.read_tx(tx_id).await?.context("Missing tx")?;

        assert_eq!(tx.tx_hash.unwrap().0, tx_hash_2);
        assert_eq!(tx.status, Some(TxStatus::Mined));

        // Destructive reorg
        let txs = &[];

        db.save_block(block_number, chain_id, timestamp, txs)
            .await?;

        full_update(&db, chain_id, finalized_timestamp).await?;

        let tx = db.read_tx(tx_id).await?.context("Missing tx")?;

        assert_eq!(tx.tx_hash.unwrap().0, tx_hash_2);
        assert_eq!(tx.status, Some(TxStatus::Pending));

        // Finalization
        let txs = &[tx_hash_2];

        db.save_block(block_number, chain_id, timestamp, txs)
            .await?;

        let finalized_timestamp = ymd_hms(2023, 11, 23, 22, 0, 0);
        full_update(&db, chain_id, finalized_timestamp).await?;

        let tx = db.read_tx(tx_id).await?.context("Missing tx")?;

        assert_eq!(tx.tx_hash.unwrap().0, tx_hash_2);
        assert_eq!(tx.status, Some(TxStatus::Finalized));

        Ok(())
    }

    #[tokio::test]
    async fn second_broadcast_of_a_tx_fails() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let relayer_id = create_relayer(&db).await?;
        create_tx(&db, &relayer_id, "tx_id").await?;

        let fee = U256::from(1);
        db.insert_tx_broadcast(
            "tx_id",
            B256::with_last_byte(1),
            &[1],
            fee,
            fee,
        )
        .await?;
        let second_broadcast = db
            .insert_tx_broadcast(
                "tx_id",
                B256::with_last_byte(2),
                &[2],
                fee,
                fee,
            )
            .await;

        assert!(second_broadcast.is_err());
        assert_eq!(tx_hashes(&db, "tx_id").await?, [B256::with_last_byte(1)]);

        Ok(())
    }

    #[tokio::test]
    async fn concurrent_escalations_store_one_replacement() -> eyre::Result<()>
    {
        let (db, _db_container) = setup_db().await?;
        let tx_id = create_broadcast_tx(&db).await?;

        let escalations = (2..10u8).map(|hash_byte| {
            let (db, tx_id) = (&db, &tx_id);

            async move {
                db.escalate_tx(
                    tx_id,
                    0,
                    B256::with_last_byte(hash_byte),
                    &[hash_byte],
                    U256::from(hash_byte) * U256::from(10),
                    U256::from(hash_byte),
                )
                .await
            }
        });
        let results = futures::future::try_join_all(escalations).await?;

        assert_eq!(results.iter().filter(|stored| **stored).count(), 1);

        let hashes = tx_hashes(&db, &tx_id).await?;
        assert_eq!(hashes.len(), 2);
        let replacement = hashes[1];

        let (escalation_count, valid_tx_hash): (i64, H256Wrapper) =
            sqlx::query_as(
                "SELECT escalation_count, valid_tx_hash FROM sent_transactions WHERE tx_id = $1",
            )
            .bind(&tx_id)
            .fetch_one(&db.pool)
            .await?;
        assert_eq!(escalation_count, 1);
        assert_eq!(valid_tx_hash.0, replacement);

        let (raw_signed_tx, max_fee_per_gas, max_priority_fee_per_gas, escalated): (
            Vec<u8>,
            HexU256Wrapper,
            HexU256Wrapper,
            bool,
        ) = sqlx::query_as(
            "SELECT raw_signed_tx, max_fee_per_gas, max_priority_fee_per_gas, escalated FROM tx_hashes WHERE tx_hash = $1",
        )
        .bind(replacement.as_slice())
        .fetch_one(&db.pool)
        .await?;
        let hash_byte = replacement[31];
        assert_eq!(raw_signed_tx, [hash_byte]);
        assert_eq!(max_fee_per_gas.0, U256::from(hash_byte) * U256::from(10));
        assert_eq!(max_priority_fee_per_gas.0, U256::from(hash_byte));
        assert!(!escalated);

        let (original_raw_signed_tx,): (Option<Vec<u8>>,) = sqlx::query_as(
            "SELECT raw_signed_tx FROM tx_hashes WHERE tx_hash = $1",
        )
        .bind(hashes[0].as_slice())
        .fetch_one(&db.pool)
        .await?;
        assert_eq!(original_raw_signed_tx, None);

        Ok(())
    }

    #[tokio::test]
    async fn escalation_of_a_changed_tx_is_not_stored() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let tx_id = create_broadcast_tx(&db).await?;
        let fee = U256::from(1);

        let stale_count = db
            .escalate_tx(&tx_id, 1, B256::with_last_byte(2), &[2], fee, fee)
            .await?;

        sqlx::query(
            "UPDATE sent_transactions SET status = $1 WHERE tx_id = $2",
        )
        .bind(TxStatus::Mined)
        .bind(&tx_id)
        .execute(&db.pool)
        .await?;
        let mined = db
            .escalate_tx(&tx_id, 0, B256::with_last_byte(3), &[3], fee, fee)
            .await?;

        assert!(!stale_count);
        assert!(!mined);
        assert_eq!(tx_hashes(&db, &tx_id).await?, [B256::with_last_byte(1)]);

        Ok(())
    }

    #[tokio::test]
    async fn counts_pending_txs_at_max_escalations() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let tx_id = create_broadcast_tx(&db).await?;
        let idle_relayer_id = create_relayer(&db).await?;
        let fee = U256::from(1);
        db.escalate_tx(&tx_id, 0, B256::with_last_byte(2), &[2], fee, fee)
            .await?;

        let txs_at = |max_escalations| {
            txs_at_max_escalations(&db, max_escalations, &idle_relayer_id)
        };

        assert_eq!(txs_at(2).await?, [0, 0]);
        assert_eq!(txs_at(1).await?, [0, 1]);

        sqlx::query(
            "UPDATE sent_transactions SET status = $1 WHERE tx_id = $2",
        )
        .bind(TxStatus::Mined)
        .bind(&tx_id)
        .execute(&db.pool)
        .await?;
        assert_eq!(txs_at(1).await?, [0, 0]);

        Ok(())
    }

    #[tokio::test]
    async fn read_tx() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;

        let chain_id = 123;
        let network_name = "network_name";
        let http_rpc = "http_rpc";
        let ws_rpc = "ws_rpc";

        db.upsert_network(chain_id, network_name, http_rpc, ws_rpc)
            .await?;

        let relayer_1_id = uuid();
        let relayer_1_id = relayer_1_id.as_str();
        let relayer_1_name = "relayer_1";

        let relayer_2_id = uuid();
        let relayer_2_id = relayer_2_id.as_str();
        let relayer_2_name = "relayer_2";

        let key_id = "key_id";
        let relayer_address = Address::with_last_byte(1);

        db.create_relayer(
            relayer_1_id,
            relayer_1_name,
            chain_id,
            key_id,
            relayer_address,
        )
        .await?;

        db.create_relayer(
            relayer_2_id,
            relayer_2_name,
            chain_id,
            key_id,
            relayer_address,
        )
        .await?;

        let tx_id = "tx_id";
        let to = Address::with_last_byte(1);
        let data: &[u8] = &[];
        let value = U256::from(0);
        let gas_limit = U256::from(0);
        let priority = TransactionPriority::Regular;
        let blobs = None;

        db.create_transaction(
            tx_id,
            to,
            data,
            value,
            gas_limit,
            priority,
            blobs,
            relayer_1_id,
        )
        .await?;

        let tx = db.read_tx(tx_id).await?;
        assert!(tx.is_some(), "Tx can be read via read_tx");

        let tx = db.read_relayer_tx(relayer_2_id, tx_id).await?;
        assert!(tx.is_none(), "Tx cannot be read by relayer 2");

        let txs = db.read_relayer_txs(relayer_2_id, None).await?;
        assert!(txs.is_empty(), "Txs cannot be read by relayer 2");

        let txs = db.read_relayer_txs(relayer_1_id, None).await?;
        assert_eq!(txs.len(), 1, "Txs can be read by relayer 1");

        Ok(())
    }

    #[tokio::test]
    async fn blocks() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;

        let block_numbers = [0, 1];
        let chain_id = 1;
        let timestamp = ymd_hms(2023, 11, 23, 12, 32, 2);
        let txs = &[
            B256::with_last_byte(1),
            B256::with_last_byte(2),
            B256::with_last_byte(3),
        ];

        db.save_block(block_numbers[0], chain_id, timestamp, txs)
            .await?;

        db.save_block(block_numbers[1], chain_id, timestamp, txs)
            .await?;

        let fee_estimates = FeesEstimate {
            base_fee_per_gas: U256::from(13_132),
            percentile_fees: vec![U256::from(0)],
        };

        let gas_price = U256::from(1_000_000_007);

        db.save_block_fees(
            block_numbers[1],
            chain_id,
            &fee_estimates,
            gas_price,
        )
        .await?;

        let latest_block_number =
            db.get_latest_block_number(chain_id)
                .await?
                .context("Could not get latest block number")?;
        let block_fees = db.get_latest_block_fees_by_chain_id(chain_id).await?;
        let block_fees = block_fees.context("Missing fees")?;

        assert_eq!(latest_block_number, block_numbers[1]);
        assert_eq!(
            block_fees.fee_estimates.base_fee_per_gas,
            fee_estimates.base_fee_per_gas
        );
        assert_eq!(
            block_fees.fee_estimates.percentile_fees,
            fee_estimates.percentile_fees
        );
        assert_eq!(block_fees.gas_price, gas_price);

        Ok(())
    }

    #[tokio::test]
    async fn wrappers_round_trip_as_raw_bytes() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;

        let u256_cases = [
            (U256::from(0u64), [0u8; 32]),
            (U256::from(0x0102u64), u256_be_bytes(0x0102)),
            (U256::MAX, [0xff; 32]),
        ];

        for (value, expected_bytes) in u256_cases {
            let (hex_raw, hex_decoded, decimal_raw, decimal_decoded): (
                Vec<u8>,
                HexU256Wrapper,
                Vec<u8>,
                DecimalU256Wrapper,
            ) = sqlx::query_as("SELECT $1, $1, $2, $2")
                .bind(HexU256Wrapper(value))
                .bind(DecimalU256Wrapper(value))
                .fetch_one(&db.pool)
                .await?;

            assert_eq!(hex_raw, expected_bytes);
            assert_eq!(hex_decoded.0, value);
            assert_eq!(decimal_raw, expected_bytes);
            assert_eq!(decimal_decoded.0, value);
        }

        let hash = H256Wrapper(HASH.into());
        let (hash_raw, hash_decoded, hashes_raw): (
            Vec<u8>,
            H256Wrapper,
            Vec<Vec<u8>>,
        ) = sqlx::query_as("SELECT $1, $1, $2")
            .bind(&hash)
            .bind(&[hash.clone(), hash.clone()][..])
            .fetch_one(&db.pool)
            .await?;

        assert_eq!(hash_raw, HASH);
        assert_eq!(hash_decoded, hash);
        assert_eq!(hashes_raw, vec![HASH.to_vec(), HASH.to_vec()]);

        let (address,): (AddressWrapper,) = sqlx::query_as("SELECT $1::BYTEA")
            .bind(ADDRESS.to_vec())
            .fetch_one(&db.pool)
            .await?;

        assert_eq!(address.0, Address::from(ADDRESS));

        Ok(())
    }

    #[tokio::test]
    async fn indexing_a_block_twice_keeps_its_fees_and_txs() -> eyre::Result<()>
    {
        let (db, _db_container) = setup_db().await?;
        let tx_id = create_broadcast_tx(&db).await?;
        let tx_hash = tx_hashes(&db, &tx_id).await?[0];
        let chain_id = 123;
        let timestamp = ymd_hms(2023, 11, 23, 12, 32, 2);
        let fee_estimates = FeesEstimate {
            base_fee_per_gas: U256::from(13_132),
            percentile_fees: vec![U256::from(1)],
        };

        db.save_block(1, chain_id, timestamp, &[tx_hash]).await?;
        db.save_block_fees(1, chain_id, &fee_estimates, U256::from(7))
            .await?;

        let txs = [tx_hash];
        tokio::try_join!(
            db.save_block(1, chain_id, timestamp, &txs),
            db.save_block(1, chain_id, timestamp, &txs),
        )?;
        db.save_block_fees(1, chain_id, &fee_estimates, U256::from(8))
            .await?;

        let block_fees = db
            .get_latest_block_fees_by_chain_id(chain_id)
            .await?
            .context("Missing fees")?;
        assert_eq!(block_fees.gas_price, U256::from(8));

        let block_txs: Vec<(H256Wrapper,)> = sqlx::query_as(
            "SELECT tx_hash FROM block_txs WHERE block_number = 1 AND chain_id = $1",
        )
        .bind(chain_id as i64)
        .fetch_all(&db.pool)
        .await?;
        assert_eq!(block_txs, [(H256Wrapper(tx_hash),)]);

        Ok(())
    }

    #[tokio::test]
    async fn persisted_value_encodings() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;

        let chain_id = 123;
        db.upsert_network(chain_id, "network_name", "http_rpc", "ws_rpc")
            .await?;

        let relayer_id = uuid();
        let address = Address::from(ADDRESS);
        db.create_relayer(&relayer_id, "relayer", chain_id, "key_id", address)
            .await?;
        db.update_relayer(
            &relayer_id,
            &RelayerUpdateRequest {
                relayer_name: None,
                max_inflight_txs: None,
                max_queued_txs: None,
                gas_price_limits: Some(vec![RelayerGasPriceLimitResponse {
                    chain_id: 1,
                    value: U256::from(10_123u64).into(),
                }]),
                enabled: None,
            },
        )
        .await?;

        let tx_id = "tx_id";
        db.create_transaction(
            tx_id,
            address,
            &[0xde, 0xad],
            U256::from(1_000_000_000_000_000_000u64),
            U256::from(21_000u64),
            TransactionPriority::Regular,
            None,
            &relayer_id,
        )
        .await?;

        let tx_hash = B256::from(HASH);
        db.insert_tx_broadcast(
            tx_id,
            tx_hash,
            &[0x02, 0xf8],
            U256::from(2_000_000_000u64),
            U256::from(1_000_000_000u64),
        )
        .await?;

        let block_timestamp = ymd_hms(2023, 11, 23, 12, 32, 2);
        db.save_block(1, chain_id, block_timestamp, &[tx_hash])
            .await?;
        db.save_block_fees(
            1,
            chain_id,
            &FeesEstimate {
                base_fee_per_gas: U256::from(13_132u64),
                percentile_fees: vec![
                    U256::from(0u64),
                    U256::from(100_000_000u64),
                ],
            },
            U256::from(1_000_000_007u64),
        )
        .await?;

        let (relayer_address, gas_price_limits): (Vec<u8>, serde_json::Value) =
            sqlx::query_as(
                "SELECT address, gas_price_limits::JSONB FROM relayers WHERE id = $1",
            )
            .bind(&relayer_id)
            .fetch_one(&db.pool)
            .await?;

        assert_eq!(relayer_address, ADDRESS);
        assert_eq!(
            gas_price_limits,
            json!([{ "chainId": 1, "value": "10123" }])
        );

        let (tx_to, data, value, gas_limit): (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) =
            sqlx::query_as(
                "SELECT tx_to, data, value, gas_limit FROM transactions WHERE id = $1",
            )
            .bind(tx_id)
            .fetch_one(&db.pool)
            .await?;

        assert_eq!(tx_to, ADDRESS);
        assert_eq!(data, [0xde, 0xad]);
        assert_eq!(value, u256_be_bytes(1_000_000_000_000_000_000));
        assert_eq!(gas_limit, u256_be_bytes(21_000));

        let (hash, raw_signed_tx, max_fee_per_gas, max_priority_fee_per_gas): (
            Vec<u8>,
            Vec<u8>,
            Vec<u8>,
            Vec<u8>,
        ) = sqlx::query_as(
            "SELECT tx_hash, raw_signed_tx, max_fee_per_gas, max_priority_fee_per_gas FROM tx_hashes WHERE tx_id = $1",
        )
        .bind(tx_id)
        .fetch_one(&db.pool)
        .await?;

        assert_eq!(hash, HASH);
        assert_eq!(raw_signed_tx, [0x02, 0xf8]);
        assert_eq!(max_fee_per_gas, u256_be_bytes(2_000_000_000));
        assert_eq!(max_priority_fee_per_gas, u256_be_bytes(1_000_000_000));

        let (
            initial_max_fee_per_gas,
            initial_max_priority_fee_per_gas,
            valid_tx_hash,
        ): (Vec<u8>, Vec<u8>, Vec<u8>) = sqlx::query_as(
            "SELECT initial_max_fee_per_gas, initial_max_priority_fee_per_gas, valid_tx_hash FROM sent_transactions WHERE tx_id = $1",
        )
        .bind(tx_id)
        .fetch_one(&db.pool)
        .await?;

        assert_eq!(initial_max_fee_per_gas, u256_be_bytes(2_000_000_000));
        assert_eq!(
            initial_max_priority_fee_per_gas,
            u256_be_bytes(1_000_000_000)
        );
        assert_eq!(valid_tx_hash, HASH);

        let (block_tx_hash,): (Vec<u8>,) =
            sqlx::query_as("SELECT tx_hash FROM block_txs WHERE chain_id = $1")
                .bind(chain_id as i64)
                .fetch_one(&db.pool)
                .await?;

        assert_eq!(block_tx_hash, HASH);

        let (gas_price, fee_estimate): (String, serde_json::Value) =
            sqlx::query_as(
                "SELECT gas_price::TEXT, fee_estimate::JSONB FROM block_fees WHERE chain_id = $1",
            )
            .bind(chain_id as i64)
            .fetch_one(&db.pool)
            .await?;

        assert_eq!(gas_price, "1000000007");
        assert_eq!(
            fee_estimate,
            json!({
                "base_fee_per_gas": "0x334c",
                "percentile_fees": ["0x0", "0x5f5e100"],
            })
        );

        Ok(())
    }

    /// Creates a network with a relayer and returns the relayer id.
    async fn create_relayer(db: &Database) -> eyre::Result<String> {
        let chain_id = 123;
        db.upsert_network(chain_id, "network_name", "http_rpc", "ws_rpc")
            .await?;

        let relayer_id = uuid();
        db.create_relayer(
            &relayer_id,
            "relayer_name",
            chain_id,
            "key_id",
            Address::with_last_byte(1),
        )
        .await?;

        Ok(relayer_id)
    }

    async fn create_tx(
        db: &Database,
        relayer_id: &str,
        tx_id: &str,
    ) -> eyre::Result<CreateResult> {
        db.create_transaction(
            tx_id,
            Address::with_last_byte(1),
            &[],
            U256::ZERO,
            U256::from(21_000),
            TransactionPriority::Regular,
            None,
            relayer_id,
        )
        .await
    }

    /// Returns the counts of [`Database::count_txs_at_max_escalations`], that
    /// of `first_relayer_id` first.
    async fn txs_at_max_escalations(
        db: &Database,
        max_escalations: usize,
        first_relayer_id: &str,
    ) -> eyre::Result<Vec<u64>> {
        let mut relayers =
            db.count_txs_at_max_escalations(max_escalations).await?;
        relayers.sort_by_key(|relayer| relayer.relayer_id != first_relayer_id);

        Ok(relayers.into_iter().map(|relayer| relayer.txs).collect())
    }

    /// Creates a transaction broadcast with hash `0x..01` and returns its id.
    async fn create_broadcast_tx(db: &Database) -> eyre::Result<String> {
        let relayer_id = create_relayer(db).await?;
        let tx_id = "tx_id".to_string();
        create_tx(db, &relayer_id, &tx_id).await?;

        let fee = U256::from(1);
        db.insert_tx_broadcast(&tx_id, B256::with_last_byte(1), &[1], fee, fee)
            .await?;

        Ok(tx_id)
    }

    /// Returns the hashes stored for `tx_id`, oldest first.
    async fn tx_hashes(db: &Database, tx_id: &str) -> eyre::Result<Vec<B256>> {
        let hashes: Vec<(H256Wrapper,)> = sqlx::query_as(
            "SELECT tx_hash FROM tx_hashes WHERE tx_id = $1 ORDER BY created_at, tx_hash",
        )
        .bind(tx_id)
        .fetch_all(&db.pool)
        .await?;

        Ok(hashes.into_iter().map(|(hash,)| hash.0).collect())
    }

    /// Big-endian 32-byte encoding of a U256 that fits in a `u64`.
    fn u256_be_bytes(value: u64) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        bytes[24..].copy_from_slice(&value.to_be_bytes());
        bytes
    }

    fn ymd_hms(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
    ) -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(hour, minute, second)
            .unwrap()
            .and_utc()
    }

    fn uuid() -> String {
        uuid::Uuid::new_v4().to_string()
    }
}
