#![allow(clippy::too_many_arguments)]

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use alloy::primitives::{Address, B256, U256};
use chrono::{DateTime, Utc};
use eyre::{Context, ContextCompat};
use sqlx::migrate::{MigrateDatabase, Migrator};
use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use sqlx::types::{BigDecimal, Json};
use sqlx::{
    Connection, FromRow, PgConnection, PgExecutor, Pool, Postgres, Row,
};
use tracing::instrument;

use crate::broadcast_utils::gas_estimation::FeesEstimate;
use crate::config::{DatabaseConfig, NetworkConfig, RelayerConfig};
use crate::db::data::{
    ApiKeyStatus, ConfigChange, NetworkInfo, Reconciliation, RelayerBacklog,
    RelayerGasPriceLimit, RelayerInfo,
};
use crate::db::wrappers::h256::H256Wrapper;
use crate::db::wrappers::hex_u256::HexU256Wrapper;
use crate::db::wrappers::xid8::Xid8;
use crate::static_config::{self, ConfigVersion, Plan};
use crate::tx_events::EventCursor;
use crate::types::{TransactionPriority, TxMetadata, TxStatus};

pub mod data;
pub mod wrappers;

use self::data::{
    BlockFees, NetworkStats, ReadTxData, RelayerStaleTxs,
    RelayerTxsAtMaxEscalations, RpcKind, TxEventData, TxEventsLag,
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

/// How long a statement on [`Database::pool`] may run, including the time it
/// waits for locks
const STATEMENT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a connection of [`Database::pool`] may sit in an open transaction
/// without running a statement before Postgres closes it.
///
/// An open transaction holds back the event feed, see
/// [`Database::read_tx_events`].
const IDLE_IN_TRANSACTION_TIMEOUT: Duration = Duration::from_secs(30);

/// The settings every connection of [`Database::pool`] asks for
const POOL_TIMEOUTS: [(&str, Duration); 2] = [
    ("statement_timeout", STATEMENT_TIMEOUT),
    (
        "idle_in_transaction_session_timeout",
        IDLE_IN_TRANSACTION_TIMEOUT,
    ),
];

/// The channel that storing tx events notifies, with the relayer id as the
/// payload. The `tx_events_notify` trigger sends it.
const TX_EVENTS_CHANNEL: &str = "tx_events";

/// The TCP keepalive settings of the listener's connection, with which
/// Postgres closes it about 90s after it died. A dead listener that Postgres
/// keeps holds back its notification queue, and storing events fails once
/// the queue is full.
const LISTENER_KEEPALIVES: [(&str, &str); 3] = [
    ("tcp_keepalives_idle", "60"),
    ("tcp_keepalives_interval", "10"),
    ("tcp_keepalives_count", "3"),
];

pub struct Database {
    /// Connections with [`POOL_TIMEOUTS`]
    pub pool: Pool<Postgres>,
    /// Reserved for [`Database::ping`]
    probe_pool: Pool<Postgres>,
}

/// The outcome of [`Database::create_transaction`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateResult {
    Created,
    /// An identical request already created the transaction, see
    /// [`TxIdMatch::SameRequest`]
    Repeat,
    /// A different request already created a transaction with the same id,
    /// see [`TxIdMatch::DifferentRequest`]
    Conflict,
    /// The relayer already has `max_queued_txs` unsent or pending
    /// transactions
    QueueFull,
    /// Another transaction of the relayer holds the relayer's next nonce,
    /// which a relayer reset repairs
    NonceTaken {
        nonce: u64,
    },
}

/// The outcome of [`Database::read_tx_events`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxEventsPage {
    Events {
        events: Vec<TxEventData>,
        /// The cursor the events after these follow
        next_cursor: EventCursor,
    },
    /// Pruning deleted events after the cursor, which the reader may not
    /// have seen
    CursorExpired,
    /// The cursor is ahead of every database transaction that ran. It is
    /// from another database, or from before a restore to an earlier point,
    /// and the reader may miss the events stored since.
    CursorAhead,
}

/// How a send request relates to the transaction stored under its id
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxIdMatch {
    /// No transaction has the id
    Unused,
    /// The transaction was created by an identical request
    SameRequest,
    /// The transaction was created by a different request, or by a request
    /// that stored no request hash, such as any v1 send
    DifferentRequest,
}

impl Database {
    pub async fn new(config: &DatabaseConfig) -> eyre::Result<Self> {
        let connection_string = config.to_connection_string();

        if !Postgres::database_exists(&connection_string).await? {
            tracing::info!("Creating database");
            Postgres::create_database(&connection_string).await?;
        }

        let connect_options: PgConnectOptions = connection_string.parse()?;

        // Migrations may take longer than the statement timeout, so they run
        // on a connection of their own
        tracing::info!("Running migrations");
        let mut connection =
            PgConnection::connect_with(&connect_options).await?;
        MIGRATOR.run(&mut connection).await?;
        connection.close().await?;

        tracing::info!("Connecting to the database");
        let pool_options = connect_options.clone().options(POOL_TIMEOUTS.map(
            |(setting, timeout)| {
                (setting, format!("{}ms", timeout.as_millis()))
            },
        ));
        let pool = PgPoolOptions::new().connect_with(pool_options).await?;
        check_timeouts(&pool).await?;

        let probe_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy_with(connect_options);

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
    pub async fn get_relayers(&self) -> eyre::Result<Vec<RelayerInfo>> {
        select_relayers(&self.pool).await
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

    /// Counts the unsent and pending transactions of `relayer_id`, leaving
    /// out [stale ones](Database::count_stale_txs), which a reset or the
    /// chain settles.
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
            JOIN relayers r ON (t.relayer_id = r.id)
            LEFT JOIN sent_transactions s ON (t.id = s.tx_id)
            WHERE t.relayer_id = $1
            AND t.nonce >= r.current_nonce
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
    /// unless `tx_id` exists, the relayer's queue is full or the nonce is
    /// taken.
    ///
    /// `request_hash` identifies the request, see [`TxIdMatch`]. An existing
    /// `tx_id` is checked first, so a repeat is answered even when the queue
    /// is full.
    #[instrument(skip(self, metadata), level = "debug")]
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
        metadata: Option<&TxMetadata>,
        request_hash: Option<[u8; 32]>,
    ) -> eyre::Result<CreateResult> {
        let mut tx = self.pool.begin().await?;

        let value_bytes: [u8; 32] = value.to_be_bytes();
        let gas_limit_bytes: [u8; 32] = gas_limit.to_be_bytes();

        // Locks the relayer, so that concurrent creates see each other's
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

        let existing =
            match_tx_id(tx.as_mut(), tx_id, relayer_id, request_hash).await?;
        if existing != TxIdMatch::Unused {
            tx.rollback().await?;

            return Ok(match existing {
                TxIdMatch::SameRequest => CreateResult::Repeat,
                _ => CreateResult::Conflict,
            });
        }

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
            INSERT INTO transactions (id, tx_to, data, value, gas_limit, priority, relayer_id, nonce, blobs, metadata, request_hash)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
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
        .bind(metadata)
        .bind(request_hash)
        .execute(tx.as_mut())
        .await;

        if let Err(sqlx::Error::Database(ref err)) = res {
            match err.constraint() {
                // Another relayer's create took the id after the check above
                Some("transactions_pkey") => return Ok(CreateResult::Conflict),
                Some("unique_relayer_nonce") => {
                    return Ok(CreateResult::NonceTaken {
                        nonce: nonce as u64,
                    })
                }
                _ => {}
            }
        }

        res?;

        tx.commit().await?;

        Ok(CreateResult::Created)
    }

    /// Compares a send request of `relayer_id` to the transaction stored
    /// under `tx_id`.
    #[instrument(skip(self), level = "debug")]
    pub async fn compare_tx_request(
        &self,
        tx_id: &str,
        relayer_id: &str,
        request_hash: [u8; 32],
    ) -> eyre::Result<TxIdMatch> {
        match_tx_id(&self.pool, tx_id, relayer_id, Some(request_hash)).await
    }

    /// Returns the transactions of enabled relayers that were never broadcast
    /// and fit in their relayer's in-flight window.
    ///
    /// Transactions whose nonce the chain already used are left out, see
    /// [`Database::count_stale_txs`].
    #[instrument(skip(self), level = "debug")]
    pub async fn get_unsent_txs(&self) -> eyre::Result<Vec<UnsentTx>> {
        Ok(sqlx::query_as(
            r#"
            SELECT     r.id as relayer_id, t.id, t.tx_to, t.data, t.value, t.gas_limit, t.priority, t.nonce, t.blobs, r.key_id, r.chain_id
            FROM       transactions t
            LEFT JOIN  sent_transactions s ON (t.id = s.tx_id)
            INNER JOIN relayers r ON (t.relayer_id = r.id)
            WHERE      s.tx_id IS NULL
            AND        r.enabled
            AND        t.nonce >= r.current_nonce
            AND        (t.nonce - r.current_nonce < r.max_inflight_txs)
            ORDER BY   r.id, t.nonce ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await?)
    }

    /// Records the first signed version of `tx_id`, which must be stored
    /// before it is sent, with its `pending` event.
    ///
    /// Fails if `tx_id` was already broadcast, so that two broadcasts of the
    /// same transaction can't both be sent, or if a reset deleted it.
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

        // A reset of the relayer waits for this, or this waits for the reset
        // and then fails if the reset deleted the transaction
        sqlx::query(
            r#"
            SELECT 1
            FROM   relayers
            WHERE  id = (SELECT relayer_id FROM transactions WHERE id = $1)
            FOR KEY SHARE
            "#,
        )
        .bind(tx_id)
        .execute(tx.as_mut())
        .await?;

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
            INSERT INTO sent_transactions (tx_id, initial_max_fee_per_gas, initial_max_priority_fee_per_gas, valid_tx_hash, last_event_xid)
            VALUES ($1, $2, $3, $4, pg_current_xact_id())
            "#
        )
        .bind(tx_id)
        .bind(initial_max_fee_per_gas_bytes)
        .bind(initial_max_priority_fee_per_gas_bytes)
        .bind(tx_hash.as_slice())
        .execute(tx.as_mut()).await?;

        insert_pending_tx_event(tx.as_mut(), tx_id, tx_hash).await?;

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
        let block = self.get_latest_block(chain_id).await?;

        Ok(block.map(|(block_number, _)| block_number))
    }

    /// Returns the number and timestamp of the latest indexed block of
    /// `chain_id`.
    #[instrument(skip(self), level = "debug")]
    pub async fn get_latest_block(
        &self,
        chain_id: u64,
    ) -> eyre::Result<Option<(u64, DateTime<Utc>)>> {
        let block: Option<(i64, DateTime<Utc>)> = sqlx::query_as(
            r#"
            SELECT block_number, timestamp
            FROM   blocks
            WHERE  chain_id = $1
            ORDER BY block_number DESC
            LIMIT  1
            "#,
        )
        .bind(chain_id as i64)
        .fetch_optional(&self.pool)
        .await?;

        Ok(block
            .map(|(block_number, timestamp)| (block_number as u64, timestamp)))
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

    /// Points every mined transaction whose valid hash isn't the one in a
    /// block at the hash that is, with a `mined` event, and returns the ids of
    /// those it changed.
    ///
    /// Like every status change that stores an event, it skips a transaction
    /// whose last event a newer database transaction stored, so that the feed
    /// can't order its event before that one. The next call changes it.
    #[instrument(skip(self), level = "debug", ret)]
    pub async fn handle_soft_reorgs(&self) -> eyre::Result<Vec<String>> {
        let tx_ids: Vec<(String,)> = sqlx::query_as(
            r#"
            WITH reorged AS (
                UPDATE    sent_transactions s
                SET       valid_tx_hash = h.tx_hash,
                          last_event_xid = pg_current_xact_id()
                FROM      tx_hashes h
                JOIN      block_txs bt ON h.tx_hash = bt.tx_hash
                WHERE     h.tx_id = s.tx_id
                AND       h.tx_hash <> s.valid_tx_hash
                AND       s.status = $1
                AND       (s.last_event_xid IS NULL OR s.last_event_xid < pg_current_xact_id())
                RETURNING s.tx_id, s.valid_tx_hash
            ),
            events AS (
                INSERT INTO tx_events (relayer_id, tx_id, status, tx_hash)
                SELECT t.relayer_id, r.tx_id, $1, r.valid_tx_hash
                FROM   reorged r
                JOIN   transactions t ON t.id = r.tx_id
            )
            SELECT tx_id FROM reorged
            "#,
        )
        .bind(TxStatus::Mined)
        .fetch_all(&self.pool)
        .await?;

        Ok(tx_ids.into_iter().map(|(tx_id,)| tx_id).collect())
    }

    /// Moves every mined transaction none of whose hashes is in a block back
    /// to pending, with its latest hash as the valid one and a `pending`
    /// event, and returns the ids of those it moved.
    ///
    /// Skips a transaction whose last event a newer database transaction
    /// stored, see [`Database::handle_soft_reorgs`].
    #[instrument(skip(self), level = "debug", ret)]
    pub async fn handle_hard_reorgs(&self) -> eyre::Result<Vec<String>> {
        let tx_ids: Vec<(String,)> = sqlx::query_as(
            r#"
            WITH reorged AS (
                UPDATE    sent_transactions s
                SET       status = $1,
                          valid_tx_hash = (
                              SELECT tx_hash
                              FROM   tx_hashes h
                              WHERE  h.tx_id = s.tx_id
                              ORDER BY created_at DESC
                              LIMIT  1
                          ),
                          mined_at = NULL,
                          last_event_xid = pg_current_xact_id()
                WHERE     s.status = $2
                AND       NOT EXISTS (
                    SELECT 1
                    FROM   tx_hashes h
                    JOIN   block_txs bt ON h.tx_hash = bt.tx_hash
                    WHERE  h.tx_id = s.tx_id
                )
                AND       (s.last_event_xid IS NULL OR s.last_event_xid < pg_current_xact_id())
                RETURNING s.tx_id, s.valid_tx_hash
            ),
            events AS (
                INSERT INTO tx_events (relayer_id, tx_id, status, tx_hash)
                SELECT t.relayer_id, r.tx_id, $1, r.valid_tx_hash
                FROM   reorged r
                JOIN   transactions t ON t.id = r.tx_id
            )
            SELECT tx_id FROM reorged
            "#,
        )
        .bind(TxStatus::Pending)
        .bind(TxStatus::Mined)
        .fetch_all(&self.pool)
        .await?;

        Ok(tx_ids.into_iter().map(|(tx_id,)| tx_id).collect())
    }

    /// Marks the pending transactions of `chain_id` with a hash in a block as
    /// mined, with a `mined` event, and returns their ids and mined hashes.
    ///
    /// Skips a transaction whose last event a newer database transaction
    /// stored, see [`Database::handle_soft_reorgs`].
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
            ),
            mined AS (
                UPDATE    sent_transactions s
                SET       status = $3,
                          valid_tx_hash = cte.tx_hash,
                          mined_at = cte.timestamp,
                          last_event_xid = pg_current_xact_id()
                FROM      cte
                WHERE     s.tx_id = cte.id
                AND       s.status = $1
                AND       (s.last_event_xid IS NULL OR s.last_event_xid < pg_current_xact_id())
                RETURNING s.tx_id, s.valid_tx_hash
            ),
            events AS (
                INSERT INTO tx_events (relayer_id, tx_id, status, tx_hash)
                SELECT t.relayer_id, m.tx_id, $3, m.valid_tx_hash
                FROM   mined m
                JOIN   transactions t ON t.id = m.tx_id
            )
            SELECT tx_id, valid_tx_hash FROM mined
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

    /// Marks every mined transaction whose valid hash is in a block older
    /// than `finalization_timestamp` as finalized, with a `finalized` event,
    /// and returns the ids of those it marked.
    ///
    /// Skips a transaction whose last event a newer database transaction
    /// stored, see [`Database::handle_soft_reorgs`].
    #[instrument(skip(self), level = "debug", ret)]
    pub async fn finalize_txs(
        &self,
        finalization_timestamp: DateTime<Utc>,
    ) -> eyre::Result<Vec<String>> {
        let tx_ids: Vec<(String,)> = sqlx::query_as(
            r#"
            WITH finalized AS (
                UPDATE    sent_transactions s
                SET       status = $1,
                          last_event_xid = pg_current_xact_id()
                FROM      block_txs bt
                JOIN      blocks b ON bt.block_number = b.block_number AND bt.chain_id = b.chain_id
                WHERE     bt.tx_hash = s.valid_tx_hash
                AND       s.status = $2
                AND       b.timestamp < $3
                AND       (s.last_event_xid IS NULL OR s.last_event_xid < pg_current_xact_id())
                RETURNING s.tx_id, s.valid_tx_hash
            ),
            events AS (
                INSERT INTO tx_events (relayer_id, tx_id, status, tx_hash)
                SELECT t.relayer_id, f.tx_id, $1, f.valid_tx_hash
                FROM   finalized f
                JOIN   transactions t ON t.id = f.tx_id
            )
            SELECT tx_id FROM finalized
            "#,
        )
        .bind(TxStatus::Finalized)
        .bind(TxStatus::Mined)
        .bind(finalization_timestamp)
        .fetch_all(&self.pool)
        .await?;

        Ok(tx_ids.into_iter().map(|(tx_id,)| tx_id).collect())
    }

    /// Returns the pending transactions whose latest signed version was
    /// stored more than `escalation_interval` ago, however often they were
    /// escalated.
    ///
    /// Transactions whose nonce the chain already used are left out, see
    /// [`Database::count_stale_txs`].
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
            AND    t.nonce >= r.current_nonce
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
    /// `escalation_count` earlier escalations, with a `pending` event. It must
    /// be stored before it is sent. The versions it replaces keep their
    /// hashes, so that indexing still finds them mined, but drop their signed
    /// bytes, which are never sent again.
    ///
    /// Returns `false`, and changes nothing, unless `tx_id` is still pending
    /// with exactly `escalation_count` escalations, and no newer database
    /// transaction stored its last event. Then another escalation or a status
    /// change won the race, and this replacement must not be sent.
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
                   updated_at = now(),
                   last_event_xid = pg_current_xact_id()
            WHERE  tx_id = $1
            AND    escalation_count = $2
            AND    status = $3
            AND    (last_event_xid IS NULL OR last_event_xid < pg_current_xact_id())
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

        insert_pending_tx_event(tx.as_mut(), tx_id, tx_hash).await?;

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
            SELECT t.id as tx_id, t.relayer_id, r.chain_id, t.tx_to as to, t.data, t.value,
                   t.gas_limit, t.nonce, t.priority, t.blobs, t.metadata, t.created_at,
                   h.tx_hash, s.status, s.mined_at::TIMESTAMPTZ as mined_at
            FROM transactions t
            JOIN relayers r ON t.relayer_id = r.id
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
            SELECT t.id as tx_id, t.relayer_id, r.chain_id, t.tx_to as to, t.data, t.value,
                   t.gas_limit, t.nonce, t.priority, t.blobs, t.metadata, t.created_at,
                   h.tx_hash, s.status, s.mined_at::TIMESTAMPTZ as mined_at
            FROM transactions t
            JOIN relayers r ON t.relayer_id = r.id
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
            SELECT t.id as tx_id, t.relayer_id, r.chain_id, t.tx_to as to, t.data, t.value,
                   t.gas_limit, t.nonce, t.priority, t.blobs, t.metadata, t.created_at,
                   h.tx_hash, s.status, s.mined_at::TIMESTAMPTZ as mined_at
            FROM transactions t
            JOIN relayers r ON t.relayer_id = r.id
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
            SELECT t.id as tx_id, t.relayer_id, r.chain_id, t.tx_to as to, t.data, t.value,
                   t.gas_limit, t.nonce, t.priority, t.blobs, t.metadata, t.created_at,
                   h.tx_hash, s.status, s.mined_at::TIMESTAMPTZ as mined_at
            FROM transactions t
            JOIN relayers r ON t.relayer_id = r.id
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

    /// Returns up to `limit` of the events of relayer `relayer_id` that follow
    /// `after`, or that follow [`EventCursor::START`] without a cursor, in
    /// feed order, and the cursor to read on from.
    ///
    /// The feed orders events by the database transaction that stored them,
    /// then by their id. It holds back the events of a transaction while an
    /// older one is still running anywhere on the database server: a
    /// transaction can take a lower id and commit after one with a higher id,
    /// and holding back keeps it from landing before an event the feed
    /// already served. A transaction that stays open stalls the feed, which
    /// [`Database::get_tx_events_lag`] shows.
    ///
    /// The next cursor follows the last event returned. When there are no
    /// more events to return, it follows every event the feed holds back none
    /// of, so that a relayer's cursor keeps up with the feed while it has no
    /// events and outlives the pruning of other relayers' events.
    ///
    /// An event's metadata is `None` once its transaction was pruned.
    #[instrument(skip(self), level = "debug")]
    pub async fn read_tx_events(
        &self,
        relayer_id: &str,
        after: Option<EventCursor>,
        limit: u32,
    ) -> eyre::Result<TxEventsPage> {
        let start = after.unwrap_or(EventCursor::START);

        // Always one row, with NULL event columns when there are no events,
        // so that the snapshot the events were read in comes back too
        let rows = sqlx::query(
            r#"
            WITH snapshot AS (
                SELECT pg_snapshot_xmin(pg_current_snapshot()) AS xmin,
                       pg_snapshot_xmax(pg_current_snapshot()) AS xmax
            )
            SELECT    s.xmin AS snapshot_xmin,
                      s.xmax AS snapshot_xmax,
                      EXISTS (
                          SELECT 1
                          FROM   tx_events_pruned p
                          WHERE  (p.tx_xid, p.id) > ($2, $3)
                      ) AS is_pruned_after_start,
                      e.*
            FROM      snapshot s
            LEFT JOIN LATERAL (
                SELECT    e.tx_xid, e.id, e.tx_id, e.relayer_id, r.chain_id,
                          e.status, e.tx_hash, t.metadata, e.created_at
                FROM      tx_events e
                JOIN      relayers r ON r.id = e.relayer_id
                LEFT JOIN transactions t
                          ON t.id = e.tx_id AND t.relayer_id = e.relayer_id
                WHERE     e.relayer_id = $1
                AND       (e.tx_xid, e.id) > ($2, $3)
                AND       e.tx_xid < s.xmin
                ORDER BY  e.tx_xid, e.id
                LIMIT     $4
            ) e ON true
            ORDER BY  e.tx_xid, e.id
            "#,
        )
        .bind(relayer_id)
        .bind(start.tx_xid)
        .bind(start.id)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;

        let snapshot = rows.first().context("The feed returned no row")?;
        let xmin: Xid8 = snapshot.try_get("snapshot_xmin")?;
        let xmax: Xid8 = snapshot.try_get("snapshot_xmax")?;
        let is_pruned_after_start: bool =
            snapshot.try_get("is_pruned_after_start")?;

        if after.is_some_and(|after| after.tx_xid > xmax) {
            return Ok(TxEventsPage::CursorAhead);
        }
        if after.is_some() && is_pruned_after_start {
            return Ok(TxEventsPage::CursorExpired);
        }

        let mut events = vec![];
        for row in &rows {
            if row.try_get::<Option<i64>, _>("id")?.is_some() {
                events.push(TxEventData::from_row(row)?);
            }
        }

        let last_read = events.last().map_or(start, |event| event.cursor);
        let next_cursor = if events.len() < limit as usize {
            last_read.max(EventCursor::before_tx(xmin))
        } else {
            last_read
        };

        Ok(TxEventsPage::Events {
            events,
            next_cursor,
        })
    }

    /// Connects a listener for the notifications that storing events of a
    /// relayer sends, with the relayer id as their payload.
    ///
    /// Every listener has a connection of its own. Notifications sent while
    /// it isn't connected are lost, and [`PgListener::try_recv`] returns
    /// `Ok(None)` or an error once the connection is.
    pub async fn listen_tx_events(&self) -> eyre::Result<PgListener> {
        let connect_options = self
            .pool
            .connect_options()
            .as_ref()
            .clone()
            .options(LISTENER_KEEPALIVES);
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy_with(connect_options);

        let mut listener = PgListener::connect_with(&pool).await?;
        listener.listen(TX_EVENTS_CHANNEL).await?;

        Ok(listener)
    }

    /// Records `nonce`, the on-chain transaction count of `relayer_address`,
    /// as the confirmed nonce of its relayers on `chain_id`, and moves their
    /// local nonce up to it.
    ///
    /// Returns the relayers whose local nonce was behind `nonce`, with that
    /// local nonce. Their key sent transactions that tx-sitter didn't create.
    #[instrument(skip(self), level = "debug")]
    pub async fn update_relayer_nonce(
        &self,
        chain_id: u64,
        relayer_address: Address,
        nonce: u64,
    ) -> eyre::Result<Vec<(String, u64)>> {
        let mut tx = self.pool.begin().await?;

        // Locks the relayers, so that no create takes a nonce in between
        let relayers: Vec<(String, i64)> = sqlx::query_as(
            r#"
            SELECT id, nonce
            FROM   relayers
            WHERE  chain_id = $1
            AND    address = $2
            FOR NO KEY UPDATE
            "#,
        )
        .bind(chain_id as i64)
        .bind(relayer_address.as_slice())
        .fetch_all(tx.as_mut())
        .await?;

        sqlx::query(
            r#"
            UPDATE relayers
            SET    current_nonce = $3,
                   nonce = GREATEST(nonce, $3),
                   updated_at = now()
            WHERE  chain_id = $1
            AND    address = $2
            "#,
        )
        .bind(chain_id as i64)
        .bind(relayer_address.as_slice())
        .bind(nonce as i64)
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;

        Ok(relayers
            .into_iter()
            .map(|(id, local_nonce)| (id, local_nonce as u64))
            .filter(|(_, local_nonce)| *local_nonce < nonce)
            .collect())
    }

    /// Counts, for every relayer, the transactions that aren't mined but
    /// whose nonce is below the relayer's confirmed nonce.
    ///
    /// The chain used those nonces for other transactions, sent with the
    /// relayer's key outside tx-sitter, so they can never be mined.
    #[instrument(skip(self), level = "debug")]
    pub async fn count_stale_txs(&self) -> eyre::Result<Vec<RelayerStaleTxs>> {
        Ok(sqlx::query_as(
            r#"
            SELECT    r.id AS relayer_id, r.chain_id, r.current_nonce, COUNT(t.id) AS stale_txs
            FROM      relayers r
            LEFT JOIN transactions t ON (
                          t.relayer_id = r.id
                      AND t.nonce < r.current_nonce
                      AND NOT EXISTS (
                          SELECT 1
                          FROM   sent_transactions s
                          WHERE  s.tx_id = t.id
                          AND    s.status <> $1
                      )
                  )
            GROUP BY  r.id
            "#,
        )
        .bind(TxStatus::Pending)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Deletes up to `batch_size` of the blocks older than `timestamp`, with
    /// their transactions and fees, and returns how many it deleted.
    #[instrument(skip(self), level = "debug")]
    pub async fn prune_blocks(
        &self,
        timestamp: DateTime<Utc>,
        batch_size: u64,
    ) -> eyre::Result<u64> {
        let pruned = sqlx::query(
            r#"
            DELETE FROM blocks
            WHERE  (block_number, chain_id) IN (
                       SELECT block_number, chain_id
                       FROM   blocks
                       WHERE  timestamp < $1
                       LIMIT  $2
                   )
            "#,
        )
        .bind(timestamp)
        .bind(batch_size as i64)
        .execute(&self.pool)
        .await?;

        Ok(pruned.rows_affected())
    }

    /// Deletes up to `batch_size` of the transactions finalized in a block
    /// mined before `timestamp`, with their hashes, and returns how many it
    /// deleted.
    #[instrument(skip(self), level = "debug")]
    pub async fn prune_txs(
        &self,
        timestamp: DateTime<Utc>,
        batch_size: u64,
    ) -> eyre::Result<u64> {
        let pruned = sqlx::query(
            r#"
            DELETE FROM transactions
            WHERE  id IN (
                       SELECT tx_id
                       FROM   sent_transactions
                       WHERE  mined_at < $1
                       AND    status = $2
                       LIMIT  $3
                   )
            "#,
        )
        .bind(timestamp)
        .bind(TxStatus::Finalized)
        .bind(batch_size as i64)
        .execute(&self.pool)
        .await?;

        Ok(pruned.rows_affected())
    }

    /// Deletes up to `batch_size` of the events stored before `timestamp`,
    /// and returns how many it deleted.
    ///
    /// It records the last position in the feed it deleted, so that the feed
    /// answers a cursor before it with [`TxEventsPage::CursorExpired`].
    #[instrument(skip(self), level = "debug")]
    pub async fn prune_tx_events(
        &self,
        timestamp: DateTime<Utc>,
        batch_size: u64,
    ) -> eyre::Result<u64> {
        let (pruned,): (i64,) = sqlx::query_as(
            r#"
            WITH pruned AS (
                DELETE FROM tx_events
                WHERE  id IN (
                           SELECT id
                           FROM   tx_events
                           WHERE  created_at < $1
                           LIMIT  $2
                       )
                RETURNING tx_xid, id
            ),
            last_pruned AS (
                INSERT INTO tx_events_pruned (tx_xid, id)
                SELECT tx_xid, id
                FROM   (
                           SELECT   tx_xid, id
                           FROM     pruned
                           ORDER BY tx_xid DESC, id DESC
                           LIMIT    1
                       ) last
                ON CONFLICT (singleton) DO UPDATE
                SET    tx_xid = EXCLUDED.tx_xid,
                       id = EXCLUDED.id
                WHERE  (tx_events_pruned.tx_xid, tx_events_pruned.id)
                     < (EXCLUDED.tx_xid, EXCLUDED.id)
            )
            SELECT COUNT(*) FROM pruned
            "#,
        )
        .bind(timestamp)
        .bind(batch_size as i64)
        .fetch_one(&self.pool)
        .await?;

        Ok(pruned as u64)
    }

    /// Returns how far the event feed is behind, see [`TxEventsLag`].
    #[instrument(skip(self), level = "debug")]
    pub async fn get_tx_events_lag(&self) -> eyre::Result<TxEventsLag> {
        Ok(sqlx::query_as(
            r#"
            SELECT COALESCE(
                       (
                           SELECT   EXTRACT(EPOCH FROM now() - created_at)::FLOAT8
                           FROM     tx_events
                           WHERE    tx_xid >= (SELECT pg_snapshot_xmin(pg_current_snapshot()))
                           ORDER BY tx_xid
                           LIMIT    1
                       ),
                       0
                   ) AS held_back_seconds,
                   pg_notification_queue_usage() AS notification_queue_usage
            "#,
        )
        .fetch_one(&self.pool)
        .await?)
    }

    /// Returns the version of the static config the database applied last,
    /// or `None` if that config had no revision.
    #[instrument(skip(self), level = "debug")]
    pub async fn get_static_config_version(
        &self,
    ) -> eyre::Result<Option<ConfigVersion>> {
        select_static_config_version(&self.pool).await
    }

    /// Makes the networks, relayers and API keys in the database match
    /// `networks` and `relayers`, and records `version` as the applied
    /// config, unless [`static_config::plan`] finds that the database already
    /// holds `version`.
    ///
    /// - Networks are upserted, and `rpcs` gets the first HTTP RPC and the WS
    ///   RPC of each, which previous versions read and `config dump` prints.
    ///   Networks missing from `networks` are left as they are.
    /// - Relayers are upserted and keep their nonces.
    /// - Relayers missing from `relayers` are disabled rather than deleted,
    ///   because their transactions reference them, and lose their API keys.
    /// - Every relayer in `relayers` accepts exactly the API keys of its
    ///   `api_key_hashes`.
    ///
    /// Everything happens in one transaction under an advisory lock, so
    /// services that start at once apply their configs one after another,
    /// and applying the same config again changes nothing.
    ///
    /// Fails without changing anything when:
    /// - [`static_config::plan`] refuses `version`;
    /// - a relayer that has transactions or allocated nonces would change its
    ///   chain id or address, because its nonces belong to them;
    /// - an enabled relayer would share its chain id and address with another
    ///   relayer that is enabled or has unmined transactions, because both
    ///   would allocate nonces of one account;
    /// - `relayers` is empty while the database has enabled relayers, the
    ///   state of a config nobody migrated yet.
    #[instrument(skip_all)]
    pub async fn apply_static_config(
        &self,
        networks: &[NetworkConfig],
        relayers: &[RelayerConfig],
        version: Option<ConfigVersion>,
    ) -> eyre::Result<Reconciliation> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("SET LOCAL lock_timeout = '10s'")
            .execute(tx.as_mut())
            .await?;
        sqlx::query("SET LOCAL statement_timeout = '30s'")
            .execute(tx.as_mut())
            .await?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(STATIC_CONFIG_LOCK_ID)
            .execute(tx.as_mut())
            .await
            .context("Waiting for the static config lock")?;

        let stored_version = select_static_config_version(tx.as_mut()).await?;
        if static_config::plan(stored_version, version)? == Plan::Skip {
            return Ok(Reconciliation::AlreadyApplied);
        }

        if relayers.is_empty() {
            let (enabled_relayers,): (i64,) =
                sqlx::query_as("SELECT COUNT(*) FROM relayers WHERE enabled")
                    .fetch_one(tx.as_mut())
                    .await?;

            if enabled_relayers > 0 {
                eyre::bail!(
                    "The config defines no relayers, but the database has \
                     {enabled_relayers} enabled relayers. Run `tx-sitter \
                     config dump` to write them into the config."
                );
            }
        }

        let relayer_ids: Vec<String> =
            relayers.iter().map(|relayer| relayer.id.clone()).collect();
        let before = ConfigState::read(tx.as_mut()).await?;

        let stored_identities: Vec<StoredIdentity> = sqlx::query_as(
            r#"
            SELECT id,
                   chain_id,
                   address,
                   nonce > 0 OR EXISTS (
                       SELECT 1 FROM transactions t WHERE t.relayer_id = relayers.id
                   ) AS has_history
            FROM   relayers
            WHERE  id = ANY($1)
            "#,
        )
        .bind(&relayer_ids)
        .fetch_all(tx.as_mut())
        .await?;
        let changed_identities =
            changed_relayer_identities(&stored_identities, relayers);
        if !changed_identities.is_empty() {
            eyre::bail!(
                "A relayer's chain id and address can't change once it has \
                 transactions, add a new relayer instead: {}",
                changed_identities.join("; ")
            );
        }

        for network in networks {
            let http_rpc = network
                .http_rpcs
                .first()
                .context("Network without HTTP RPCs")?;

            upsert_network_in(
                tx.as_mut(),
                network.chain_id,
                &network.name,
                http_rpc.expose().as_str(),
                network.ws_rpc.expose().as_str(),
            )
            .await
            .with_context(|| {
                format!("Upserting network {}", network.chain_id)
            })?;
        }

        for relayer in relayers {
            upsert_relayer_in(tx.as_mut(), relayer)
                .await
                .with_context(|| format!("Upserting relayer {}", relayer.id))?;
        }

        sqlx::query(
            r#"
            UPDATE relayers
            SET    enabled = FALSE
            WHERE  enabled
            AND    NOT (id = ANY($1))
            "#,
        )
        .bind(&relayer_ids)
        .execute(tx.as_mut())
        .await?;

        replace_api_keys_in(tx.as_mut(), relayers).await?;

        let shared_accounts =
            shared_accounts_in(tx.as_mut(), &relayer_ids).await?;
        if !shared_accounts.is_empty() {
            eyre::bail!(
                "Relayers would allocate the same nonces, because they have \
                 the same chain id and address and the second one is enabled \
                 or has unmined transactions: {}",
                shared_accounts
                    .iter()
                    .map(|(relayer_id, other_id)| format!(
                        "{relayer_id} and {other_id}"
                    ))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }

        store_static_config_version_in(tx.as_mut(), version).await?;

        let after = ConfigState::read(tx.as_mut()).await?;

        tx.commit().await?;

        Ok(Reconciliation::Applied(config_changes(
            &before,
            &after,
            &relayer_ids,
        )))
    }

    #[instrument(skip(self), level = "debug")]
    pub async fn get_networks(&self) -> eyre::Result<Vec<NetworkInfo>> {
        select_networks(&self.pool).await
    }

    /// Returns whether relayer `relayer_id` accepts the API key whose secret
    /// hashes to `api_key_hash`.
    #[instrument(skip(self), level = "debug")]
    pub async fn get_api_key_status(
        &self,
        relayer_id: &str,
        api_key_hash: [u8; 32],
    ) -> eyre::Result<ApiKeyStatus> {
        let enabled: Option<(bool,)> = sqlx::query_as(
            r#"
            SELECT r.enabled
            FROM   api_keys k
            JOIN   relayers r ON (k.relayer_id = r.id)
            WHERE  k.relayer_id = $1
            AND    k.key_hash = $2
            LIMIT  1
            "#,
        )
        .bind(relayer_id)
        .bind(api_key_hash)
        .fetch_optional(&self.pool)
        .await?;

        Ok(match enabled {
            None => ApiKeyStatus::Unknown,
            Some((false,)) => ApiKeyStatus::RelayerDisabled,
            Some((true,)) => ApiKeyStatus::Valid,
        })
    }

    /// Returns the disabled relayers that still have transactions they
    /// haven't sent or seen mined.
    #[instrument(skip(self), level = "debug")]
    pub async fn get_disabled_relayer_backlogs(
        &self,
    ) -> eyre::Result<Vec<RelayerBacklog>> {
        Ok(sqlx::query_as(
            r#"
            SELECT   r.id AS relayer_id,
                     r.name AS relayer_name,
                     COUNT(*) FILTER (WHERE s.tx_id IS NULL) AS unsent_txs,
                     COUNT(*) FILTER (WHERE s.status = $1) AS inflight_txs
            FROM     relayers r
            JOIN     transactions t ON (t.relayer_id = r.id)
            LEFT JOIN sent_transactions s ON (t.id = s.tx_id)
            WHERE    NOT r.enabled
            AND      (s.tx_id IS NULL OR s.status = $1)
            GROUP BY r.id, r.name
            ORDER BY r.id
            "#,
        )
        .bind(TxStatus::Pending)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Returns the hashes of the API keys of every relayer, by relayer id.
    #[instrument(skip(self), level = "debug")]
    pub async fn get_api_key_hashes(
        &self,
    ) -> eyre::Result<Vec<(String, B256)>> {
        let rows: Vec<(String, Vec<u8>)> = sqlx::query_as(
            r#"
            SELECT   relayer_id, key_hash
            FROM     api_keys
            ORDER BY relayer_id, key_hash
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|(relayer_id, key_hash)| {
                let key_hash = B256::try_from(key_hash.as_slice())
                    .context("API key hash is not 32 bytes")?;

                Ok((relayer_id, key_hash))
            })
            .collect()
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

    /// Deletes the transactions of `relayer_id` that were never broadcast,
    /// and moves its nonce back to the first one that no remaining
    /// transaction holds.
    ///
    /// The nonce never goes below the on-chain nonce or the nonce of a
    /// broadcast transaction, so no nonce is used twice.
    #[instrument(skip(self), level = "debug")]
    pub async fn purge_unsent_txs(&self, relayer_id: &str) -> eyre::Result<()> {
        let mut tx = self.pool.begin().await?;

        // Waits for, and then blocks, creates and broadcasts of this relayer
        sqlx::query(
            r#"
            SELECT 1
            FROM   relayers
            WHERE  id = $1
            FOR UPDATE
            "#,
        )
        .bind(relayer_id)
        .execute(tx.as_mut())
        .await?;

        sqlx::query(
            r#"
            DELETE FROM transactions t
            WHERE  t.relayer_id = $1
            AND    NOT EXISTS (
                SELECT 1 FROM sent_transactions s WHERE s.tx_id = t.id
            )
            "#,
        )
        .bind(relayer_id)
        .execute(tx.as_mut())
        .await?;

        sqlx::query(
            r#"
            UPDATE relayers r
            SET    nonce = GREATEST(
                       r.current_nonce,
                       COALESCE(
                           (
                               SELECT MAX(t.nonce) + 1
                               FROM   transactions t
                               JOIN   sent_transactions s ON s.tx_id = t.id
                               WHERE  t.relayer_id = r.id
                           ),
                           0
                       )
                   ),
                   updated_at = now()
            WHERE  r.id = $1
            "#,
        )
        .bind(relayer_id)
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;

        Ok(())
    }
}

/// Fails unless the connections of `pool` have [`POOL_TIMEOUTS`].
///
/// A connection pooler such as PgBouncer with `ignore_startup_parameters`
/// drops the timeouts a connection asks for without an error.
async fn check_timeouts(pool: &Pool<Postgres>) -> eyre::Result<()> {
    for (setting, timeout) in POOL_TIMEOUTS {
        let (timeout_ms,): (String,) =
            sqlx::query_as("SELECT setting FROM pg_settings WHERE name = $1")
                .bind(setting)
                .fetch_one(pool)
                .await?;
        let expected_ms = timeout.as_millis().to_string();

        eyre::ensure!(
            timeout_ms == expected_ms,
            "The database connections have a {setting} of {timeout_ms}ms \
             instead of {expected_ms}ms, is a connection pooler dropping the \
             options startup parameter?"
        );
    }

    Ok(())
}

/// Records that `tx_id` is pending with valid hash `tx_hash`
async fn insert_pending_tx_event(
    executor: impl PgExecutor<'_>,
    tx_id: &str,
    tx_hash: B256,
) -> eyre::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO tx_events (relayer_id, tx_id, status, tx_hash)
        SELECT relayer_id, id, $2, $3
        FROM   transactions
        WHERE  id = $1
        "#,
    )
    .bind(tx_id)
    .bind(TxStatus::Pending)
    .bind(tx_hash.as_slice())
    .execute(executor)
    .await?;

    Ok(())
}

async fn match_tx_id(
    executor: impl PgExecutor<'_>,
    tx_id: &str,
    relayer_id: &str,
    request_hash: Option<[u8; 32]>,
) -> eyre::Result<TxIdMatch> {
    let is_same_request: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT COALESCE(relayer_id = $2 AND request_hash = $3, FALSE)
        FROM   transactions
        WHERE  id = $1
        "#,
    )
    .bind(tx_id)
    .bind(relayer_id)
    .bind(request_hash)
    .fetch_optional(executor)
    .await?;

    Ok(match is_same_request {
        None => TxIdMatch::Unused,
        Some((true,)) => TxIdMatch::SameRequest,
        Some((false,)) => TxIdMatch::DifferentRequest,
    })
}

async fn count_queued_txs(
    executor: impl PgExecutor<'_>,
    relayer_id: &str,
) -> eyre::Result<usize> {
    let (tx_count,): (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(1)
        FROM transactions t
        JOIN relayers r ON (t.relayer_id = r.id)
        LEFT JOIN sent_transactions s ON (t.id = s.tx_id)
        WHERE t.relayer_id = $1
        AND t.nonce >= r.current_nonce
        AND (s.tx_id IS NULL OR s.status = $2)
        "#,
    )
    .bind(relayer_id)
    .bind(TxStatus::Pending)
    .fetch_one(executor)
    .await?;

    Ok(tx_count as usize)
}

/// The advisory lock that serialises [`Database::apply_static_config`]
const STATIC_CONFIG_LOCK_ID: i64 = 0x7478_7369_7474_6572;

async fn select_relayers<'e>(
    executor: impl PgExecutor<'e>,
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
        ORDER BY id
        "#,
    )
    .fetch_all(executor)
    .await?)
}

async fn select_networks<'e>(
    executor: impl PgExecutor<'e>,
) -> eyre::Result<Vec<NetworkInfo>> {
    Ok(sqlx::query_as(
        r#"
        SELECT networks.chain_id, name, http.url as http_rpc, ws.url as ws_rpc
        FROM   networks
        INNER JOIN rpcs http ON networks.chain_id = http.chain_id AND http.kind = 'http'
        INNER JOIN rpcs ws ON networks.chain_id = ws.chain_id AND ws.kind = 'ws'
        ORDER BY networks.chain_id
        "#,
    )
    .fetch_all(executor)
    .await?)
}

async fn select_static_config_version<'e>(
    executor: impl PgExecutor<'e>,
) -> eyre::Result<Option<ConfigVersion>> {
    let row: Option<(i64, Vec<u8>)> =
        sqlx::query_as("SELECT revision, hash FROM static_config_version")
            .fetch_optional(executor)
            .await?;

    row.map(|(revision, hash)| {
        Ok(ConfigVersion {
            revision: u64::try_from(revision)
                .context("Stored config revision is negative")?,
            hash: B256::try_from(hash.as_slice())
                .context("Stored config hash is not 32 bytes")?,
        })
    })
    .transpose()
}

/// Records `version` as the version of the applied config, or forgets the
/// stored version when `version` is `None`, so that the next config with a
/// revision is applied whatever its revision.
async fn store_static_config_version_in(
    conn: &mut PgConnection,
    version: Option<ConfigVersion>,
) -> eyre::Result<()> {
    let Some(version) = version else {
        sqlx::query("DELETE FROM static_config_version")
            .execute(conn)
            .await?;

        return Ok(());
    };

    let revision = i64::try_from(version.revision)
        .context("Config revision doesn't fit in a BIGINT")?;
    sqlx::query(
        r#"
        INSERT INTO static_config_version (revision, hash)
        VALUES ($1, $2)
        ON CONFLICT (singleton) DO UPDATE
        SET revision = EXCLUDED.revision,
            hash = EXCLUDED.hash,
            applied_at = now()
        "#,
    )
    .bind(revision)
    .bind(version.hash.as_slice())
    .execute(conn)
    .await?;

    Ok(())
}

async fn upsert_network_in(
    conn: &mut PgConnection,
    chain_id: u64,
    name: &str,
    http_rpc: &str,
    ws_rpc: &str,
) -> eyre::Result<()> {
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
    .execute(&mut *conn)
    .await?;

    for (url, kind) in [(http_rpc, RpcKind::Http), (ws_rpc, RpcKind::Ws)] {
        sqlx::query(
            r#"
            INSERT INTO rpcs (chain_id, url, kind)
            VALUES ($1, $2, $3)
            ON CONFLICT (chain_id, kind) DO UPDATE
            SET url = EXCLUDED.url
            "#,
        )
        .bind(chain_id as i64)
        .bind(url)
        .bind(kind)
        .execute(&mut *conn)
        .await?;
    }

    Ok(())
}

/// The chain id and address of a stored relayer, and whether it has used
/// them
#[derive(sqlx::FromRow)]
struct StoredIdentity {
    id: String,
    chain_id: i64,
    address: Vec<u8>,
    /// Whether the relayer has transactions or allocated nonces
    has_history: bool,
}

/// Describes every relayer of `relayers` whose chain id or address differs
/// from its stored one although it has a history.
fn changed_relayer_identities(
    stored_identities: &[StoredIdentity],
    relayers: &[RelayerConfig],
) -> Vec<String> {
    stored_identities
        .iter()
        .filter(|stored| stored.has_history)
        .filter_map(|stored| {
            let relayer =
                relayers.iter().find(|relayer| relayer.id == stored.id)?;
            let stored_address =
                Address::try_from(stored.address.as_slice()).ok();

            let is_unchanged = relayer.chain_id as i64 == stored.chain_id
                && stored_address == Some(relayer.address);

            (!is_unchanged).then(|| {
                format!(
                    "relayer {} has chain id {} and address 0x{} in the \
                     database, but chain id {} and address {} in the config",
                    stored.id,
                    stored.chain_id,
                    hex::encode(&stored.address),
                    relayer.chain_id,
                    relayer.address
                )
            })
        })
        .collect()
}

/// Inserts `relayer`, or updates the settings of the stored relayer with its
/// id, leaving its nonces as they are.
async fn upsert_relayer_in(
    conn: &mut PgConnection,
    relayer: &RelayerConfig,
) -> eyre::Result<()> {
    let gas_price_limits: Vec<RelayerGasPriceLimit> = relayer
        .gas_price_limits
        .iter()
        .map(|limit| RelayerGasPriceLimit {
            value: limit.value.clone().into(),
            chain_id: limit.chain_id as i64,
        })
        .collect();

    sqlx::query(
        r#"
        INSERT INTO relayers (
            id, name, chain_id, key_id, address,
            max_inflight_txs, max_queued_txs, gas_price_limits, enabled
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        ON CONFLICT (id) DO UPDATE
        SET name = EXCLUDED.name,
            chain_id = EXCLUDED.chain_id,
            key_id = EXCLUDED.key_id,
            address = EXCLUDED.address,
            max_inflight_txs = EXCLUDED.max_inflight_txs,
            max_queued_txs = EXCLUDED.max_queued_txs,
            gas_price_limits = EXCLUDED.gas_price_limits,
            enabled = EXCLUDED.enabled
        "#,
    )
    .bind(&relayer.id)
    .bind(&relayer.name)
    .bind(relayer.chain_id as i64)
    .bind(relayer.key.key_id())
    .bind(relayer.address.as_slice())
    .bind(relayer.max_inflight_txs as i64)
    .bind(relayer.max_queued_txs as i64)
    .bind(Json(gas_price_limits))
    .bind(relayer.enabled)
    .execute(conn)
    .await?;

    Ok(())
}

/// Makes the stored API keys exactly the `api_key_hashes` of `relayers`, so
/// relayers missing from `relayers` lose theirs.
async fn replace_api_keys_in(
    conn: &mut PgConnection,
    relayers: &[RelayerConfig],
) -> eyre::Result<()> {
    let (key_relayer_ids, key_hashes): (Vec<&str>, Vec<&[u8]>) = relayers
        .iter()
        .flat_map(|relayer| {
            relayer
                .api_key_hashes
                .iter()
                .map(|hash| (relayer.id.as_str(), hash.as_slice()))
        })
        .unzip();

    sqlx::query("DELETE FROM api_keys")
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO api_keys (relayer_id, key_hash)
        SELECT DISTINCT * FROM UNNEST($1::TEXT[], $2::BYTEA[])
        "#,
    )
    .bind(&key_relayer_ids)
    .bind(&key_hashes)
    .execute(&mut *conn)
    .await?;

    Ok(())
}

/// Returns `(relayer, other)` id pairs where `relayer` is one of
/// `relayer_ids` and enabled, and `other` has the same chain id and address
/// and is enabled or has unmined transactions.
async fn shared_accounts_in(
    conn: &mut PgConnection,
    relayer_ids: &[String],
) -> eyre::Result<Vec<(String, String)>> {
    Ok(sqlx::query_as(
        r#"
        SELECT   r.id, other.id
        FROM     relayers r
        JOIN     relayers other
                 ON  other.chain_id = r.chain_id
                 AND other.address = r.address
                 AND other.id <> r.id
        WHERE    r.id = ANY($1)
        AND      r.enabled
        AND      (other.enabled OR EXISTS (
                     SELECT    1
                     FROM      transactions t
                     LEFT JOIN sent_transactions s ON (t.id = s.tx_id)
                     WHERE     t.relayer_id = other.id
                     AND       (s.tx_id IS NULL OR s.status = $2)
                 ))
        ORDER BY r.id, other.id
        "#,
    )
    .bind(relayer_ids)
    .bind(TxStatus::Pending)
    .fetch_all(conn)
    .await?)
}

/// The rows the static config writes
struct ConfigState {
    networks: Vec<NetworkInfo>,
    relayers: Vec<RelayerInfo>,
    /// `(relayer_id, key_hash)` rows
    api_keys: Vec<(String, Vec<u8>)>,
}

impl ConfigState {
    async fn read(conn: &mut PgConnection) -> eyre::Result<Self> {
        Ok(Self {
            networks: select_networks(&mut *conn).await?,
            relayers: select_relayers(&mut *conn).await?,
            api_keys: sqlx::query_as(
                "SELECT relayer_id, key_hash FROM api_keys",
            )
            .fetch_all(&mut *conn)
            .await?,
        })
    }
}

/// Lists what differs between `before` and `after`, where `configured_ids`
/// are the ids of the relayers in the config.
fn config_changes(
    before: &ConfigState,
    after: &ConfigState,
    configured_ids: &[String],
) -> Vec<ConfigChange> {
    fn changed_fields<const N: usize>(
        fields: [(&'static str, bool); N],
    ) -> Vec<&'static str> {
        fields
            .into_iter()
            .filter_map(|(field, is_changed)| is_changed.then_some(field))
            .collect()
    }

    let mut changes = vec![];

    for network in &after.networks {
        let chain_id = network.chain_id;
        let Some(old) = before.networks.iter().find(|n| n.chain_id == chain_id)
        else {
            changes.push(ConfigChange::NetworkAdded { chain_id });
            continue;
        };

        let fields = changed_fields([
            ("name", old.name != network.name),
            ("http_rpc", old.http_rpc != network.http_rpc),
            ("ws_rpc", old.ws_rpc != network.ws_rpc),
        ]);
        if !fields.is_empty() {
            changes.push(ConfigChange::NetworkChanged { chain_id, fields });
        }
    }

    for relayer in &after.relayers {
        let relayer_id = relayer.id.clone();
        let relayer_name = relayer.name.clone();
        let Some(old) = before.relayers.iter().find(|r| r.id == relayer.id)
        else {
            changes.push(ConfigChange::RelayerAdded {
                relayer_id,
                relayer_name,
            });
            continue;
        };

        let fields = changed_fields([
            ("name", old.name != relayer.name),
            ("chain_id", old.chain_id != relayer.chain_id),
            ("key_id", old.key_id != relayer.key_id),
            ("address", old.address != relayer.address),
            (
                "max_inflight_txs",
                old.max_inflight_txs != relayer.max_inflight_txs,
            ),
            (
                "max_queued_txs",
                old.max_queued_txs != relayer.max_queued_txs,
            ),
            (
                "gas_price_limits",
                old.gas_price_limits != relayer.gas_price_limits,
            ),
        ]);
        if !fields.is_empty() {
            changes.push(ConfigChange::RelayerChanged {
                relayer_id: relayer_id.clone(),
                relayer_name: relayer_name.clone(),
                fields,
            });
        }

        match (old.enabled, relayer.enabled) {
            (false, true) => changes.push(ConfigChange::RelayerEnabled {
                relayer_id,
                relayer_name,
            }),
            (true, false) if configured_ids.contains(&relayer.id) => changes
                .push(ConfigChange::RelayerDisabled {
                    relayer_id,
                    relayer_name,
                }),
            (true, false) => changes.push(ConfigChange::RelayerRemoved {
                relayer_id,
                relayer_name,
            }),
            (true, true) | (false, false) => {}
        }
    }

    let keys_by_relayer = |state: &ConfigState| {
        let mut keys: BTreeMap<String, BTreeSet<Vec<u8>>> = BTreeMap::new();
        for (relayer_id, key_hash) in &state.api_keys {
            keys.entry(relayer_id.clone())
                .or_default()
                .insert(key_hash.clone());
        }

        keys
    };
    let keys_before = keys_by_relayer(before);
    let keys_after = keys_by_relayer(after);
    let relayer_ids: BTreeSet<&String> =
        keys_before.keys().chain(keys_after.keys()).collect();
    let no_keys = BTreeSet::new();
    for relayer_id in relayer_ids {
        let before = keys_before.get(relayer_id).unwrap_or(&no_keys);
        let after = keys_after.get(relayer_id).unwrap_or(&no_keys);

        let added = after.difference(before).count();
        if added > 0 {
            changes.push(ConfigChange::ApiKeysAdded {
                relayer_id: relayer_id.clone(),
                count: added,
            });
        }
        let revoked = before.difference(after).count();
        if revoked > 0 {
            changes.push(ConfigChange::ApiKeysRevoked {
                relayer_id: relayer_id.clone(),
                count: revoked,
            });
        }
    }

    changes
}

/// Fixtures for rows that only [`Database::apply_static_config`] writes
/// in production
#[cfg(test)]
impl Database {
    pub(crate) async fn create_relayer(
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

    pub(crate) async fn upsert_network(
        &self,
        chain_id: u64,
        name: &str,
        http_rpc: &str,
        ws_rpc: &str,
    ) -> eyre::Result<()> {
        let mut tx = self.pool.begin().await?;

        upsert_network_in(tx.as_mut(), chain_id, name, http_rpc, ws_rpc)
            .await?;

        tx.commit().await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use hex_literal::hex;
    use postgres_docker_utils::DockerContainerGuard;
    use serde_json::json;

    use super::*;
    use crate::config::{GasPriceLimitConfig, RelayerKey};
    use crate::db::wrappers::address::AddressWrapper;
    use crate::db::wrappers::decimal_u256::DecimalU256Wrapper;
    use crate::types::secret_url::SecretUrl;

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
    async fn pool_connections_have_timeouts() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;

        let (statement_timeout, idle_in_transaction_timeout): (String, String) =
            sqlx::query_as(
                "SELECT current_setting('statement_timeout'), current_setting('idle_in_transaction_session_timeout')",
            )
            .fetch_one(&db.pool)
            .await?;

        assert_eq!(statement_timeout, "30s");
        assert_eq!(idle_in_transaction_timeout, "30s");

        Ok(())
    }

    #[tokio::test]
    async fn rejects_connections_without_the_timeouts() -> eyre::Result<()> {
        let (db, db_container) = setup_db().await?;
        let url = format!(
            "postgres://postgres:postgres@{}/database",
            db_container.address()
        );
        let without_timeouts = PgPoolOptions::new().connect(&url).await?;
        let only_statement_timeout = PgPoolOptions::new()
            .connect_with(
                url.parse::<PgConnectOptions>()?
                    .options([("statement_timeout", "30000ms")]),
            )
            .await?;

        check_timeouts(&db.pool).await?;
        assert!(check_timeouts(&without_timeouts).await.is_err());
        assert!(check_timeouts(&only_statement_timeout).await.is_err());

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
    async fn reset_racing_with_sends_reuses_no_nonce() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let relayer_id = create_relayer(&db).await?;
        set_relayer_limits(&db, &relayer_id, 1_000, 2_000).await?;

        let creates = async {
            for i in 0..100 {
                create_tx(&db, &relayer_id, &format!("tx_{i}")).await?;
            }

            Ok::<_, eyre::Report>(())
        };
        // Broadcasting a transaction that a reset deleted fails
        let broadcasts = async {
            let mut broadcast_tx_ids = vec![];
            for _ in 0..50 {
                for tx in db.get_unsent_txs().await? {
                    let fee = U256::from(1);
                    let tx_hash = alloy::primitives::keccak256(&tx.id);
                    let broadcast = db
                        .insert_tx_broadcast(
                            &tx.id,
                            tx_hash,
                            tx.id.as_bytes(),
                            fee,
                            fee,
                        )
                        .await;

                    if broadcast.is_ok() {
                        broadcast_tx_ids.push(tx.id);
                    }
                }
            }

            Ok::<_, eyre::Report>(broadcast_tx_ids)
        };
        let resets = async {
            for _ in 0..20 {
                db.purge_unsent_txs(&relayer_id).await?;
                tokio::task::yield_now().await;
            }

            Ok::<_, eyre::Report>(())
        };
        let ((), broadcast_tx_ids, ()) =
            tokio::try_join!(creates, broadcasts, resets)?;

        let (sent_txs,): (i64,) = sqlx::query_as(
            "SELECT COUNT(1) FROM sent_transactions WHERE tx_id = ANY($1)",
        )
        .bind(&broadcast_tx_ids)
        .fetch_one(&db.pool)
        .await?;
        assert_eq!(sent_txs as usize, broadcast_tx_ids.len());

        let (highest_nonce,): (Option<i64>,) = sqlx::query_as(
            "SELECT MAX(nonce) FROM transactions WHERE relayer_id = $1",
        )
        .bind(&relayer_id)
        .fetch_one(&db.pool)
        .await?;
        let relayer = db
            .get_relayer(&relayer_id)
            .await?
            .context("Missing relayer")?;
        assert!(highest_nonce.is_none_or(|n| (n as u64) < relayer.nonce));

        db.purge_unsent_txs(&relayer_id).await?;
        let relayer = db
            .get_relayer(&relayer_id)
            .await?
            .context("Missing relayer")?;
        assert_eq!(relayer.nonce, broadcast_tx_ids.len() as u64);

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

        assert_eq!(db.prune_blocks(prune_timestamp, 10).await?, 1);

        assert!(!db.has_blocks_for_chain(1).await?, "Should not have blocks");

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
            None, None,
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

        assert_eq!(
            tx_event_changes(&db, tx_id).await?,
            [
                (TxStatus::Pending, Some(tx_hash_1)),
                // Escalation
                (TxStatus::Pending, Some(tx_hash_2)),
                (TxStatus::Mined, Some(tx_hash_1)),
                // Soft reorg
                (TxStatus::Mined, Some(tx_hash_2)),
                // Hard reorg
                (TxStatus::Pending, Some(tx_hash_2)),
                (TxStatus::Mined, Some(tx_hash_2)),
                (TxStatus::Finalized, Some(tx_hash_2)),
            ]
        );
        let (event_relayer_ids,): (Vec<String>,) = sqlx::query_as(
            "SELECT array_agg(DISTINCT relayer_id) FROM tx_events",
        )
        .fetch_one(&db.pool)
        .await?;
        assert_eq!(event_relayer_ids, [relayer_id]);

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
        assert_eq!(
            tx_event_changes(&db, &tx_id).await?,
            [(TxStatus::Pending, Some(B256::with_last_byte(1)))]
        );

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
    async fn create_with_a_taken_nonce_reports_it() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let relayer_id = create_relayer(&db).await?;
        create_tx(&db, &relayer_id, "first").await?;
        sqlx::query("UPDATE relayers SET nonce = 0 WHERE id = $1")
            .bind(&relayer_id)
            .execute(&db.pool)
            .await?;

        let result = create_tx(&db, &relayer_id, "second").await?;

        assert_eq!(result, CreateResult::NonceTaken { nonce: 0 });
        assert!(db.read_tx("second").await?.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn create_answers_a_repeat_before_admission() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let relayer_id = create_relayer(&db).await?;
        let other_relayer_id = create_relayer(&db).await?;
        set_max_queued_txs(&db, &relayer_id, 1).await?;
        let request_hash = [7; 32];

        let created =
            create_v2_tx(&db, &relayer_id, "tx", request_hash).await?;
        assert_eq!(created, CreateResult::Created);

        // The queue is full, but a repeat takes no slot
        for (relayer_id, request_hash, expected) in [
            (&relayer_id, Some(request_hash), CreateResult::Repeat),
            (&relayer_id, Some([8; 32]), CreateResult::Conflict),
            (&relayer_id, None, CreateResult::Conflict),
            (
                &other_relayer_id,
                Some(request_hash),
                CreateResult::Conflict,
            ),
        ] {
            let result = db
                .create_transaction(
                    "tx",
                    Address::with_last_byte(1),
                    &[],
                    U256::ZERO,
                    U256::from(21_000),
                    TransactionPriority::Regular,
                    None,
                    relayer_id,
                    None,
                    request_hash,
                )
                .await?;
            assert_eq!(result, expected, "{relayer_id} {request_hash:?}");
        }
        assert_eq!(
            create_v2_tx(&db, &relayer_id, "next", request_hash).await?,
            CreateResult::QueueFull
        );

        for (tx_id, relayer_id, request_hash, expected) in [
            ("tx", &relayer_id, request_hash, TxIdMatch::SameRequest),
            ("tx", &relayer_id, [8; 32], TxIdMatch::DifferentRequest),
            (
                "tx",
                &other_relayer_id,
                request_hash,
                TxIdMatch::DifferentRequest,
            ),
            ("next", &relayer_id, request_hash, TxIdMatch::Unused),
        ] {
            assert_eq!(
                db.compare_tx_request(tx_id, relayer_id, request_hash)
                    .await?,
                expected,
                "{tx_id} {relayer_id}"
            );
        }
        let relayer = db
            .get_relayer(&relayer_id)
            .await?
            .context("Missing relayer")?;
        assert_eq!(relayer.nonce, 1);

        Ok(())
    }

    #[tokio::test]
    async fn concurrent_creates_of_one_tx_id_store_one_tx() -> eyre::Result<()>
    {
        let (db, _db_container) = setup_db().await?;
        let relayer_id = create_relayer(&db).await?;

        // Half the requests are identical, the other half differ from them
        let request_hashes: Vec<[u8; 32]> =
            (0..10).map(|i| [i % 2; 32]).collect();
        let results = futures::future::try_join_all(request_hashes.iter().map(
            |request_hash| create_v2_tx(&db, &relayer_id, "tx", *request_hash),
        ))
        .await?;

        let created: Vec<_> = request_hashes
            .iter()
            .zip(&results)
            .filter(|(_, result)| **result == CreateResult::Created)
            .map(|(request_hash, _)| *request_hash)
            .collect();
        let [winner] = created[..] else {
            panic!("Expected a single created tx, got {results:?}");
        };
        for (request_hash, result) in request_hashes.iter().zip(&results) {
            let expected = match *request_hash == winner {
                true => [CreateResult::Created, CreateResult::Repeat],
                false => [CreateResult::Conflict, CreateResult::Conflict],
            };
            assert!(expected.contains(result), "{request_hash:?} {result:?}");
        }
        let relayer = db
            .get_relayer(&relayer_id)
            .await?
            .context("Missing relayer")?;
        assert_eq!(relayer.nonce, 1);

        Ok(())
    }

    #[tokio::test]
    async fn v2_columns_round_trip() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let relayer_id = create_relayer(&db).await?;
        let other_relayer_id = create_relayer(&db).await?;

        let metadata_json = r#"{ "b": 18446744073709551617, "a": [1.10] }"#;
        let metadata: TxMetadata = serde_json::from_str(metadata_json)?;
        let created_after = Utc::now() - chrono::Duration::seconds(5);

        db.create_transaction(
            "v2_tx",
            Address::from(ADDRESS),
            &[0xde, 0xad],
            U256::from(1_000u64),
            U256::from(21_000u64),
            TransactionPriority::Fast,
            None,
            &relayer_id,
            Some(&metadata),
            Some([7; 32]),
        )
        .await?;
        create_tx(&db, &relayer_id, "v1_tx").await?;

        assert!(db
            .read_relayer_tx(&other_relayer_id, "v2_tx")
            .await?
            .is_none());

        let tx = db
            .read_relayer_tx(&relayer_id, "v2_tx")
            .await?
            .context("Missing tx")?;
        assert_eq!(tx.relayer_id, relayer_id);
        assert_eq!(tx.chain_id, 123);
        assert_eq!(tx.priority, TransactionPriority::Fast);
        assert_eq!(
            tx.metadata.as_ref().map(TxMetadata::as_str),
            Some(metadata_json)
        );
        assert!(tx.created_at > created_after, "{}", tx.created_at);
        assert_eq!(tx.mined_at, None);

        let v1_tx = db.read_tx("v1_tx").await?.context("Missing tx")?;
        assert_eq!(v1_tx.nonce, 1);
        assert_eq!(v1_tx.metadata, None);
        assert!(v1_tx.created_at > created_after, "{}", v1_tx.created_at);

        let tx_hash = B256::from(HASH);
        let block_timestamp = ymd_hms(2023, 11, 23, 12, 32, 2);
        db.insert_tx_broadcast(
            "v2_tx",
            tx_hash,
            &[0x02],
            U256::from(2u64),
            U256::from(1u64),
        )
        .await?;
        db.save_block(1, 123, block_timestamp, &[tx_hash]).await?;
        db.mine_txs(123).await?;

        let tx = db.read_tx("v2_tx").await?.context("Missing tx")?;
        assert_eq!(tx.tx_hash.map(|hash| hash.0), Some(tx_hash));
        assert_eq!(tx.status, Some(TxStatus::Mined));
        assert_eq!(tx.mined_at, Some(block_timestamp));

        Ok(())
    }

    #[tokio::test]
    async fn finalize_reports_only_the_txs_it_finalized() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let tx_id = create_broadcast_tx(&db).await?;
        let chain_id = 123;
        let mined_at = ymd_hms(2023, 11, 23, 12, 32, 2);
        db.save_block(1, chain_id, mined_at, &[B256::with_last_byte(1)])
            .await?;
        db.mine_txs(chain_id).await?;

        // A hard reorg moves the transaction back to pending while
        // finalizing waits for its row
        let mut reorg = db.pool.begin().await?;
        sqlx::query(
            "UPDATE sent_transactions SET status = $1 WHERE tx_id = $2",
        )
        .bind(TxStatus::Pending)
        .bind(&tx_id)
        .execute(reorg.as_mut())
        .await?;
        let (finalized, committed) =
            tokio::join!(db.finalize_txs(Utc::now()), async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                reorg.commit().await
            });
        committed?;

        assert_eq!(finalized?, Vec::<String>::new());
        let tx = db.read_tx(&tx_id).await?.context("Missing tx")?;
        assert_eq!(tx.status, Some(TxStatus::Pending));
        let tx_hash = Some(B256::with_last_byte(1));
        assert_eq!(
            tx_event_changes(&db, &tx_id).await?,
            [(TxStatus::Pending, tx_hash), (TxStatus::Mined, tx_hash)]
        );

        Ok(())
    }

    #[tokio::test]
    async fn mining_leaves_a_tx_escalated_by_a_newer_transaction(
    ) -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let relayer_id = create_relayer(&db).await?;
        let chain_id = 123;
        let fee = U256::from(1);
        for (tx_id, hash_byte) in [("a", 1), ("b", 2)] {
            create_tx(&db, &relayer_id, tx_id).await?;
            let tx_hash = B256::with_last_byte(hash_byte);
            db.insert_tx_broadcast(tx_id, tx_hash, &[hash_byte], fee, fee)
                .await?;
        }
        let mined_hashes = [B256::with_last_byte(1), B256::with_last_byte(2)];
        db.save_block(1, chain_id, Utc::now(), &mined_hashes)
            .await?;

        // Mining takes its database transaction id at "a" and waits for its
        // row, while a newer transaction escalates "b" and commits first
        let mut a_lock = db.pool.begin().await?;
        sqlx::query(
            "SELECT 1 FROM sent_transactions WHERE tx_id = 'a' FOR UPDATE",
        )
        .execute(a_lock.as_mut())
        .await?;
        let (mined, escalated) = tokio::join!(db.mine_txs(chain_id), async {
            await_lock_wait(&db).await?;
            let escalation_hash = B256::with_last_byte(3);
            let escalated = db
                .escalate_tx("b", 0, escalation_hash, &[3], fee, fee)
                .await?;
            a_lock.rollback().await?;

            Ok::<_, eyre::Report>(escalated)
        });
        assert!(escalated?);
        let mined_tx_ids: Vec<_> =
            mined?.into_iter().map(|(tx_id, _)| tx_id).collect();
        assert_eq!(mined_tx_ids, ["a"]);

        assert_eq!(
            db.mine_txs(chain_id).await?,
            [("b".to_string(), mined_hashes[1])]
        );
        assert_eq!(
            tx_event_changes(&db, "b").await?,
            [
                (TxStatus::Pending, Some(B256::with_last_byte(2))),
                (TxStatus::Pending, Some(B256::with_last_byte(3))),
                (TxStatus::Mined, Some(B256::with_last_byte(2))),
            ]
        );

        Ok(())
    }

    #[tokio::test]
    async fn lag_shows_events_held_back_by_an_open_transaction(
    ) -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let relayer_id = create_relayer(&db).await?;
        for tx_id in ["a", "b"] {
            create_tx(&db, &relayer_id, tx_id).await?;
        }
        assert_eq!(db.get_tx_events_lag().await?.held_back_seconds, 0.0);

        // B commits while the older A is still open
        let mut a = db.pool.begin().await?;
        insert_pending_tx_event(a.as_mut(), "a", B256::with_last_byte(1))
            .await?;
        insert_pending_tx_event(&db.pool, "b", B256::with_last_byte(2)).await?;
        tokio::time::sleep(Duration::from_secs(1)).await;

        let lag = db.get_tx_events_lag().await?;
        assert!(lag.held_back_seconds >= 1.0, "{lag:?}");
        assert!(lag.notification_queue_usage < 0.01, "{lag:?}");

        a.commit().await?;
        assert_eq!(db.get_tx_events_lag().await?.held_back_seconds, 0.0);

        Ok(())
    }

    #[tokio::test]
    async fn pruning_records_the_last_position_it_deleted() -> eyre::Result<()>
    {
        let (db, _db_container) = setup_db().await?;
        let relayer_id = create_relayer(&db).await?;
        for i in 0..3 {
            let tx_id = format!("tx_{i}");
            create_tx(&db, &relayer_id, &tx_id).await?;
            insert_pending_tx_event(&db.pool, &tx_id, B256::with_last_byte(i))
                .await?;
        }
        let positions: Vec<(Xid8, i64)> = sqlx::query_as(
            "SELECT tx_xid, id FROM tx_events ORDER BY tx_xid, id",
        )
        .fetch_all(&db.pool)
        .await?;
        sqlx::query("UPDATE tx_events SET created_at = now() - interval '2 days' WHERE tx_id <> 'tx_2'")
            .execute(&db.pool)
            .await?;

        let prune_timestamp = Utc::now() - chrono::Duration::days(1);
        assert_eq!(db.prune_tx_events(prune_timestamp, 1).await?, 1);
        assert_eq!(db.prune_tx_events(prune_timestamp, 1).await?, 1);
        assert_eq!(db.prune_tx_events(prune_timestamp, 1).await?, 0);

        assert_eq!(tx_event_changes(&db, "tx_2").await?.len(), 1);
        assert_eq!(last_pruned(&db).await?, positions[1]);

        // The last pruned position never moves back
        let later = (Xid8(i64::MAX as u64), 0);
        sqlx::query("UPDATE tx_events_pruned SET tx_xid = $1, id = $2")
            .bind(later.0)
            .bind(later.1)
            .execute(&db.pool)
            .await?;
        sqlx::query(
            "UPDATE tx_events SET created_at = now() - interval '2 days'",
        )
        .execute(&db.pool)
        .await?;
        assert_eq!(db.prune_tx_events(prune_timestamp, 10).await?, 1);
        assert_eq!(last_pruned(&db).await?, later);

        Ok(())
    }

    #[tokio::test]
    async fn feed_holds_back_events_until_older_transactions_finish(
    ) -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let relayer_id = create_relayer(&db).await?;
        for tx_id in ["a", "b"] {
            create_tx(&db, &relayer_id, tx_id).await?;
        }

        // A takes the lower event id and commits after B
        let mut a = db.pool.begin().await?;
        insert_pending_tx_event(a.as_mut(), "a", B256::with_last_byte(1))
            .await?;
        insert_pending_tx_event(&db.pool, "b", B256::with_last_byte(2)).await?;

        assert_eq!(read_events(&db, &relayer_id, None, 100).await?.0, []);

        a.commit().await?;

        let (events, _) = read_events(&db, &relayer_id, None, 100).await?;
        let tx_ids: Vec<_> = events.iter().map(|e| e.tx_id.as_str()).collect();
        assert_eq!(tx_ids, ["a", "b"]);
        assert!(events[0].cursor < events[1].cursor);
        assert!(events[0].cursor.id < events[1].cursor.id);

        Ok(())
    }

    #[tokio::test]
    async fn feed_pages_follow_the_cursor() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let relayer_id = create_relayer(&db).await?;
        let other_relayer_id = create_relayer(&db).await?;
        let metadata: TxMetadata =
            serde_json::from_str(r#"{ "amount": 18446744073709551617 }"#)?;
        db.create_transaction(
            "with_metadata",
            Address::with_last_byte(1),
            &[],
            U256::ZERO,
            U256::from(21_000),
            TransactionPriority::Regular,
            None,
            &relayer_id,
            Some(&metadata),
            None,
        )
        .await?;
        create_tx(&db, &other_relayer_id, "other").await?;
        for (tx_id, hash_byte) in [("with_metadata", 1), ("other", 2)] {
            let fee = U256::from(1);
            db.insert_tx_broadcast(
                tx_id,
                B256::with_last_byte(hash_byte),
                &[hash_byte],
                fee,
                fee,
            )
            .await?;
        }
        db.escalate_tx(
            "with_metadata",
            0,
            B256::with_last_byte(3),
            &[3],
            U256::from(2),
            U256::from(2),
        )
        .await?;

        let (first_page, first_cursor) =
            read_events(&db, &relayer_id, None, 1).await?;
        let [first] = &first_page[..] else {
            panic!("Expected one event, got {first_page:?}");
        };
        let (second_page, second_cursor) =
            read_events(&db, &relayer_id, Some(first_cursor), 10).await?;
        let [second] = &second_page[..] else {
            panic!("Expected one event, got {second_page:?}");
        };
        let (last_page, last_cursor) =
            read_events(&db, &relayer_id, Some(second_cursor), 10).await?;

        assert_eq!(first.tx_id, "with_metadata");
        assert_eq!(first.relayer_id, relayer_id);
        assert_eq!(first.chain_id, 123);
        assert_eq!(first.status, TxStatus::Pending);
        assert_eq!(first.tx_hash, Some(H256Wrapper(B256::with_last_byte(1))));
        assert_eq!(second.tx_hash, Some(H256Wrapper(B256::with_last_byte(3))));
        assert_eq!(
            second.metadata.as_ref().map(TxMetadata::as_str),
            Some(metadata.as_str())
        );
        // A full page may be followed by more events, the others by none of
        // a transaction older than the feed holds back
        assert_eq!(first_cursor, first.cursor);
        assert!(second_cursor > second.cursor, "{second_cursor:?}");
        assert_eq!(second_cursor.id, 0);
        assert_eq!(last_page, []);
        assert_eq!(last_cursor, second_cursor);
        let (other_events, _) =
            read_events(&db, &other_relayer_id, None, 100).await?;
        assert_eq!(other_events.len(), 1);
        assert_eq!(other_events[0].tx_id, "other");

        Ok(())
    }

    #[tokio::test]
    async fn feed_refuses_cursors_before_pruned_events() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let relayer_id = create_relayer(&db).await?;
        for i in 0..3 {
            let tx_id = format!("tx_{i}");
            create_tx(&db, &relayer_id, &tx_id).await?;
            insert_pending_tx_event(&db.pool, &tx_id, B256::with_last_byte(i))
                .await?;
        }
        let (events, _) = read_events(&db, &relayer_id, None, 100).await?;
        sqlx::query("UPDATE tx_events SET created_at = now() - interval '2 days' WHERE tx_id <> 'tx_2'")
            .execute(&db.pool)
            .await?;

        let prune_timestamp = Utc::now() - chrono::Duration::days(1);
        assert_eq!(db.prune_tx_events(prune_timestamp, 1).await?, 1);
        assert_eq!(db.prune_tx_events(prune_timestamp, 1).await?, 1);
        assert_eq!(db.prune_tx_events(prune_timestamp, 1).await?, 0);

        for cursor in [EventCursor::START, events[0].cursor] {
            assert_eq!(
                db.read_tx_events(&relayer_id, Some(cursor), 10).await?,
                TxEventsPage::CursorExpired,
                "{cursor:?}"
            );
        }
        // Nothing after the last pruned event is missing
        assert_eq!(
            read_events(&db, &relayer_id, Some(events[1].cursor), 100)
                .await?
                .0,
            events[2..]
        );
        assert_eq!(
            read_events(&db, &relayer_id, None, 100).await?.0,
            events[2..]
        );
        assert_eq!(
            read_events(&db, &relayer_id, Some(events[2].cursor), 100)
                .await?
                .0,
            []
        );

        Ok(())
    }

    #[tokio::test]
    async fn idle_relayers_cursor_outlives_other_relayers_pruned_events(
    ) -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let idle_relayer_id = create_relayer(&db).await?;
        let busy_relayer_id = create_relayer(&db).await?;
        create_tx(&db, &idle_relayer_id, "idle").await?;
        insert_pending_tx_event(&db.pool, "idle", B256::with_last_byte(1))
            .await?;
        let (_, read_cursor) =
            read_events(&db, &idle_relayer_id, None, 100).await?;

        for i in 0..3 {
            let tx_id = format!("busy_{i}");
            create_tx(&db, &busy_relayer_id, &tx_id).await?;
            insert_pending_tx_event(&db.pool, &tx_id, B256::with_last_byte(i))
                .await?;
        }
        let (events, polled_cursor) =
            read_events(&db, &idle_relayer_id, Some(read_cursor), 100).await?;
        assert_eq!(events, []);
        assert!(polled_cursor > read_cursor, "{polled_cursor:?}");

        sqlx::query(
            "UPDATE tx_events SET created_at = now() - interval '2 days'",
        )
        .execute(&db.pool)
        .await?;
        let prune_timestamp = Utc::now() - chrono::Duration::days(1);
        assert_eq!(db.prune_tx_events(prune_timestamp, 100).await?, 4);

        assert_eq!(
            read_events(&db, &idle_relayer_id, Some(polled_cursor), 100)
                .await?
                .0,
            []
        );
        // It may have missed the other relayer's events
        assert_eq!(
            db.read_tx_events(&idle_relayer_id, Some(read_cursor), 100)
                .await?,
            TxEventsPage::CursorExpired
        );

        Ok(())
    }

    #[tokio::test]
    async fn feed_refuses_cursors_ahead_of_the_database() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let relayer_id = create_relayer(&db).await?;

        // No transaction runs, so the cursor is at the newest one
        let (_, cursor) = read_events(&db, &relayer_id, None, 100).await?;
        assert_eq!(
            read_events(&db, &relayer_id, Some(cursor), 100).await?.1,
            cursor
        );

        let ahead = EventCursor::before_tx(Xid8(cursor.tx_xid.0 + 1_000));
        assert_eq!(
            db.read_tx_events(&relayer_id, Some(ahead), 100).await?,
            TxEventsPage::CursorAhead
        );

        Ok(())
    }

    #[tokio::test]
    async fn events_of_a_pruned_tx_lose_its_metadata() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let relayer_id = create_relayer(&db).await?;
        let other_relayer_id = create_relayer(&db).await?;
        create_tx(&db, &relayer_id, "tx").await?;
        insert_pending_tx_event(&db.pool, "tx", B256::with_last_byte(1))
            .await?;

        // Pruning the transaction frees its id for another relayer's
        sqlx::query("DELETE FROM transactions WHERE id = 'tx'")
            .execute(&db.pool)
            .await?;
        let metadata: TxMetadata = serde_json::from_str(r#"{ "a": 1 }"#)?;
        db.create_transaction(
            "tx",
            Address::with_last_byte(1),
            &[],
            U256::ZERO,
            U256::from(21_000),
            TransactionPriority::Regular,
            None,
            &other_relayer_id,
            Some(&metadata),
            None,
        )
        .await?;

        let (events, _) = read_events(&db, &relayer_id, None, 100).await?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].metadata, None);

        Ok(())
    }

    #[tokio::test]
    async fn listener_receives_relayer_ids_of_committed_events(
    ) -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let relayer_id = create_relayer(&db).await?;
        create_tx(&db, &relayer_id, "tx").await?;
        let connections_before = client_connections(&db).await?;

        for _ in 0..3 {
            let mut listener = db.listen_tx_events().await?;
            let mut tx = db.pool.begin().await?;
            insert_pending_tx_event(tx.as_mut(), "tx", B256::with_last_byte(1))
                .await?;

            let before_commit = tokio::time::timeout(
                Duration::from_millis(200),
                listener.try_recv(),
            )
            .await;
            assert!(before_commit.is_err(), "{before_commit:?}");

            tx.commit().await?;
            let notification = listener.try_recv().await?.context("Lost")?;
            assert_eq!(notification.channel(), TX_EVENTS_CHANNEL);
            assert_eq!(notification.payload(), relayer_id);
        }

        // Dropped listeners close their connections
        for _ in 0..50 {
            if client_connections(&db).await? == connections_before {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        eyre::bail!("Dropped listeners kept their connections open");
    }

    #[tokio::test]
    async fn idle_transactions_are_closed() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let mut tx = db.pool.begin().await?;
        sqlx::query("SET LOCAL idle_in_transaction_session_timeout = '100ms'")
            .execute(tx.as_mut())
            .await?;

        tokio::time::sleep(Duration::from_millis(500)).await;

        assert!(sqlx::query("SELECT 1").execute(tx.as_mut()).await.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn stale_txs_hold_no_queue_slots() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let relayer_id = create_relayer(&db).await?;
        set_relayer_limits(&db, &relayer_id, 1, 2).await?;
        create_tx(&db, &relayer_id, "sent").await?;
        create_tx(&db, &relayer_id, "unsent").await?;
        let fee = U256::from(1);
        db.insert_tx_broadcast("sent", B256::with_last_byte(1), &[1], fee, fee)
            .await?;
        assert_eq!(
            create_tx(&db, &relayer_id, "queue_full").await?,
            CreateResult::QueueFull
        );

        // The key used both nonces outside tx-sitter
        db.update_relayer_nonce(123, Address::with_last_byte(1), 2)
            .await?;
        db.purge_unsent_txs(&relayer_id).await?;

        assert_eq!(db.get_relayer_pending_txs(&relayer_id).await?, 0);
        assert_eq!(
            db.get_relayer_pending_txs_gas_limit_sum(&relayer_id)
                .await?,
            U256::ZERO
        );
        for tx_id in ["after_reset_1", "after_reset_2"] {
            assert_eq!(
                create_tx(&db, &relayer_id, tx_id).await?,
                CreateResult::Created
            );
        }

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
            None,
            None,
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

        let block_fees = db
            .get_latest_block_fees_by_chain_id(chain_id)
            .await?
            .context("Missing fees")?;
        assert_eq!(block_fees.gas_price, U256::from(7));
        assert_eq!(
            db.get_latest_block_number_without_fee_estimates(chain_id)
                .await?,
            None
        );

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
        let address = Address::from(ADDRESS);
        let relayer = RelayerConfig {
            address,
            gas_price_limits: vec![GasPriceLimitConfig {
                chain_id: 1,
                value: U256::from(10_123u64).into(),
            }],
            ..relayer_config(chain_id, 1)
        };
        let relayer_id = relayer.id.clone();
        db.apply_static_config(&[network_config(chain_id)], &[relayer], None)
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
            None,
            None,
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

    /// Sets the queue limits of `relayer_id`, which the static config does
    /// in production.
    async fn set_relayer_limits(
        db: &Database,
        relayer_id: &str,
        max_inflight_txs: i64,
        max_queued_txs: i64,
    ) -> eyre::Result<()> {
        sqlx::query(
            r#"
            UPDATE relayers
            SET    max_inflight_txs = $2,
                   max_queued_txs = $3
            WHERE  id = $1
            "#,
        )
        .bind(relayer_id)
        .bind(max_inflight_txs)
        .bind(max_queued_txs)
        .execute(&db.pool)
        .await?;

        Ok(())
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
            None,
            None,
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

    async fn create_v2_tx(
        db: &Database,
        relayer_id: &str,
        tx_id: &str,
        request_hash: [u8; 32],
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
            None,
            Some(request_hash),
        )
        .await
    }

    async fn set_max_queued_txs(
        db: &Database,
        relayer_id: &str,
        max_queued_txs: u64,
    ) -> eyre::Result<()> {
        sqlx::query(
            r#"
            UPDATE relayers
            SET    max_inflight_txs = 0,
                   max_queued_txs = $2
            WHERE  id = $1
            "#,
        )
        .bind(relayer_id)
        .bind(max_queued_txs as i64)
        .execute(&db.pool)
        .await?;

        Ok(())
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

    /// Returns up to `limit` events of `relayer_id` after `after`, and the
    /// cursor after them.
    async fn read_events(
        db: &Database,
        relayer_id: &str,
        after: Option<EventCursor>,
        limit: u32,
    ) -> eyre::Result<(Vec<TxEventData>, EventCursor)> {
        match db.read_tx_events(relayer_id, after, limit).await? {
            TxEventsPage::Events {
                events,
                next_cursor,
            } => Ok((events, next_cursor)),
            page => eyre::bail!("Expected events, got {page:?}"),
        }
    }

    /// Returns the status and valid hash of every event stored for `tx_id`,
    /// in feed order.
    async fn tx_event_changes(
        db: &Database,
        tx_id: &str,
    ) -> eyre::Result<Vec<(TxStatus, Option<B256>)>> {
        let changes: Vec<(TxStatus, Option<H256Wrapper>)> = sqlx::query_as(
            "SELECT status, tx_hash FROM tx_events WHERE tx_id = $1 ORDER BY tx_xid, id",
        )
        .bind(tx_id)
        .fetch_all(&db.pool)
        .await?;

        Ok(changes
            .into_iter()
            .map(|(status, tx_hash)| (status, tx_hash.map(|hash| hash.0)))
            .collect())
    }

    /// Returns once a statement waits for a lock
    async fn await_lock_wait(db: &Database) -> eyre::Result<()> {
        for _ in 0..50 {
            let (is_waiting,): (bool,) = sqlx::query_as(
                "SELECT EXISTS (SELECT 1 FROM pg_locks WHERE NOT granted)",
            )
            .fetch_one(&db.pool)
            .await?;
            if is_waiting {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        eyre::bail!("No statement waits for a lock")
    }

    async fn last_pruned(db: &Database) -> eyre::Result<(Xid8, i64)> {
        Ok(sqlx::query_as("SELECT tx_xid, id FROM tx_events_pruned")
            .fetch_one(&db.pool)
            .await?)
    }

    async fn client_connections(db: &Database) -> eyre::Result<i64> {
        let (connections,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM pg_stat_activity WHERE backend_type = 'client backend'",
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(connections)
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

    fn network_config(chain_id: u64) -> NetworkConfig {
        let url = |url: &str| SecretUrl::new(url.parse().unwrap());

        NetworkConfig {
            chain_id,
            name: format!("chain {chain_id}"),
            http_rpcs: vec![
                url("https://primary.example.com/key"),
                url("https://fallback.example.com/key"),
            ],
            ws_rpc: url("wss://ws.example.com/key"),
        }
    }

    fn relayer_config(chain_id: u64, address_byte: u8) -> RelayerConfig {
        RelayerConfig {
            id: uuid(),
            name: format!("relayer {address_byte}"),
            chain_id,
            key: RelayerKey::Kms(format!("key-{address_byte}")),
            address: Address::with_last_byte(address_byte),
            max_inflight_txs: 5,
            max_queued_txs: 20,
            gas_price_limits: vec![],
            enabled: true,
            api_key_hashes: vec![B256::with_last_byte(address_byte)],
        }
    }

    async fn count_rows(db: &Database, table: &str) -> eyre::Result<i64> {
        let (count,): (i64,) =
            sqlx::query_as(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&db.pool)
                .await?;

        Ok(count)
    }

    fn version(revision: u64, hash_byte: u8) -> Option<ConfigVersion> {
        Some(ConfigVersion {
            revision,
            hash: B256::with_last_byte(hash_byte),
        })
    }

    async fn create_unsent_tx(
        db: &Database,
        relayer_id: &str,
    ) -> eyre::Result<()> {
        db.create_transaction(
            &uuid(),
            Address::ZERO,
            &[],
            U256::ZERO,
            U256::from(21_000),
            TransactionPriority::Regular,
            None,
            relayer_id,
            None,
            None,
        )
        .await?;

        Ok(())
    }

    #[tokio::test]
    async fn static_config_updates_relayers_and_keeps_their_nonces(
    ) -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let networks = [network_config(1)];
        let relayer = relayer_config(1, 1);

        db.apply_static_config(&networks, std::slice::from_ref(&relayer), None)
            .await?;
        sqlx::query(
            "UPDATE relayers SET nonce = 7, current_nonce = 5 WHERE id = $1",
        )
        .bind(&relayer.id)
        .execute(&db.pool)
        .await?;

        let updated = RelayerConfig {
            name: "renamed".to_string(),
            key: RelayerKey::Kms("arn:aws:kms:key/1".to_string()),
            max_inflight_txs: 10,
            max_queued_txs: 30,
            gas_price_limits: vec![GasPriceLimitConfig {
                chain_id: 1,
                value: U256::from(1_000).into(),
            }],
            enabled: false,
            ..relayer.clone()
        };
        db.apply_static_config(&networks, &[updated], None).await?;

        let stored =
            db.get_relayer(&relayer.id).await?.context("No relayer")?;
        assert_eq!(stored.name, "renamed");
        assert_eq!(stored.chain_id, 1);
        assert_eq!(stored.key_id, "arn:aws:kms:key/1");
        assert_eq!(stored.address.0, relayer.address);
        assert_eq!(stored.nonce, 7);
        assert_eq!(stored.current_nonce, 5);
        assert_eq!(stored.max_inflight_txs, 10);
        assert_eq!(stored.max_queued_txs, 30);
        assert_eq!(
            stored.gas_price_limits,
            vec![RelayerGasPriceLimit {
                value: U256::from(1_000).into(),
                chain_id: 1,
            }]
        );
        assert!(!stored.enabled);

        let networks = db.get_networks().await?;
        assert_eq!(networks.len(), 1);
        assert_eq!(networks[0].http_rpc, "https://primary.example.com/key");
        assert_eq!(networks[0].ws_rpc, "wss://ws.example.com/key");

        Ok(())
    }

    #[tokio::test]
    async fn static_config_reports_what_it_changed() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let a = RelayerConfig {
            id: "aaaaaaaa-0000-4000-8000-000000000000".to_string(),
            ..relayer_config(1, 1)
        };
        let b = RelayerConfig {
            id: "bbbbbbbb-0000-4000-8000-000000000000".to_string(),
            ..relayer_config(1, 2)
        };
        let relayer =
            |config: &RelayerConfig| (config.id.clone(), config.name.clone());
        let (a_id, a_name) = relayer(&a);
        let (b_id, b_name) = relayer(&b);

        let added = db
            .apply_static_config(
                &[network_config(1)],
                &[b.clone(), a.clone()],
                None,
            )
            .await?;
        assert_eq!(
            added,
            Reconciliation::Applied(vec![
                ConfigChange::NetworkAdded { chain_id: 1 },
                ConfigChange::RelayerAdded {
                    relayer_id: a_id.clone(),
                    relayer_name: a_name.clone(),
                },
                ConfigChange::RelayerAdded {
                    relayer_id: b_id.clone(),
                    relayer_name: b_name.clone(),
                },
                ConfigChange::ApiKeysAdded {
                    relayer_id: a_id.clone(),
                    count: 1,
                },
                ConfigChange::ApiKeysAdded {
                    relayer_id: b_id.clone(),
                    count: 1,
                },
            ])
        );

        let renamed_network = NetworkConfig {
            name: "renamed".to_string(),
            ..network_config(1)
        };
        let changed_a = RelayerConfig {
            max_inflight_txs: 10,
            enabled: false,
            api_key_hashes: vec![B256::with_last_byte(9)],
            ..a
        };
        let changed = db
            .apply_static_config(
                std::slice::from_ref(&renamed_network),
                std::slice::from_ref(&changed_a),
                None,
            )
            .await?;
        assert_eq!(
            changed,
            Reconciliation::Applied(vec![
                ConfigChange::NetworkChanged {
                    chain_id: 1,
                    fields: vec!["name"],
                },
                ConfigChange::RelayerChanged {
                    relayer_id: a_id.clone(),
                    relayer_name: a_name.clone(),
                    fields: vec!["max_inflight_txs"],
                },
                ConfigChange::RelayerDisabled {
                    relayer_id: a_id.clone(),
                    relayer_name: a_name,
                },
                ConfigChange::RelayerRemoved {
                    relayer_id: b_id.clone(),
                    relayer_name: b_name,
                },
                ConfigChange::ApiKeysAdded {
                    relayer_id: a_id.clone(),
                    count: 1,
                },
                ConfigChange::ApiKeysRevoked {
                    relayer_id: a_id,
                    count: 1,
                },
                ConfigChange::ApiKeysRevoked {
                    relayer_id: b_id,
                    count: 1,
                },
            ])
        );

        let unchanged = db
            .apply_static_config(&[renamed_network], &[changed_a], None)
            .await?;
        assert_eq!(unchanged, Reconciliation::Applied(vec![]));

        Ok(())
    }

    #[tokio::test]
    async fn concurrent_static_config_applications_are_idempotent(
    ) -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let networks = [network_config(1), network_config(2)];
        let relayers = [
            RelayerConfig {
                api_key_hashes: vec![
                    B256::with_last_byte(1),
                    B256::with_last_byte(2),
                ],
                ..relayer_config(1, 1)
            },
            relayer_config(2, 3),
        ];
        let version = ConfigVersion::new(1, &networks, &relayers);

        let results = futures::future::join_all((0..4).map(|_| {
            db.apply_static_config(&networks, &relayers, Some(version))
        }))
        .await;

        let mut applications = 0;
        for result in results {
            if let Reconciliation::Applied(_) = result? {
                applications += 1;
            }
        }
        assert_eq!(applications, 1);
        assert_eq!(db.get_static_config_version().await?, Some(version));
        assert_eq!(count_rows(&db, "networks").await?, 2);
        assert_eq!(count_rows(&db, "rpcs").await?, 4);
        assert_eq!(count_rows(&db, "relayers").await?, 2);
        assert_eq!(count_rows(&db, "api_keys").await?, 3);
        for relayer in &relayers {
            for hash in &relayer.api_key_hashes {
                assert_eq!(
                    db.get_api_key_status(&relayer.id, hash.0).await?,
                    ApiKeyStatus::Valid
                );
            }
        }

        Ok(())
    }

    /// Without the lock, transactions that upsert the same rows in different
    /// orders deadlock, and Postgres aborts one of them.
    #[tokio::test]
    async fn concurrent_static_configs_dont_deadlock() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let networks: Vec<NetworkConfig> =
            (1..=10).map(network_config).collect();
        let relayers: Vec<RelayerConfig> = (1..=10)
            .map(|chain_id| relayer_config(chain_id, chain_id as u8))
            .collect();
        let reversed_networks: Vec<NetworkConfig> =
            networks.iter().rev().cloned().collect();
        let reversed_relayers: Vec<RelayerConfig> =
            relayers.iter().rev().cloned().collect();

        for _ in 0..10 {
            let (forward, backward) = tokio::join!(
                db.apply_static_config(&networks, &relayers, None),
                db.apply_static_config(
                    &reversed_networks,
                    &reversed_relayers,
                    None
                ),
            );

            forward?;
            backward?;
        }

        Ok(())
    }

    #[tokio::test]
    async fn static_config_refuses_an_older_revision() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let networks = [network_config(1)];
        let relayer = relayer_config(1, 1);
        let stale = RelayerConfig {
            name: "stale".to_string(),
            ..relayer.clone()
        };

        db.apply_static_config(
            &networks,
            std::slice::from_ref(&relayer),
            version(2, 1),
        )
        .await?;
        let err = db
            .apply_static_config(&networks, &[stale], version(1, 2))
            .await
            .unwrap_err();

        assert!(
            err.to_string().contains(
                "The config has revision 1, but the database applied revision 2 last"
            ),
            "{err}"
        );
        let stored =
            db.get_relayer(&relayer.id).await?.context("No relayer")?;
        assert_eq!(stored.name, relayer.name, "The stale config was applied");
        assert_eq!(db.get_static_config_version().await?, version(2, 1));

        Ok(())
    }

    #[tokio::test]
    async fn static_config_refuses_a_changed_config_of_the_same_revision(
    ) -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let networks = [network_config(1)];
        let relayer = relayer_config(1, 1);

        db.apply_static_config(
            &networks,
            std::slice::from_ref(&relayer),
            version(1, 1),
        )
        .await?;
        let err = db
            .apply_static_config(
                &networks,
                std::slice::from_ref(&relayer),
                version(1, 2),
            )
            .await
            .unwrap_err();

        assert!(
            err.to_string().contains(
                "The config has revision 1, which the database applied with other networks or relayers"
            ),
            "{err}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn static_config_skips_the_version_it_applied() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let networks = [network_config(1)];
        let relayer = relayer_config(1, 1);

        db.apply_static_config(
            &networks,
            std::slice::from_ref(&relayer),
            version(1, 1),
        )
        .await?;
        sqlx::query("UPDATE relayers SET name = 'changed by hand'")
            .execute(&db.pool)
            .await?;
        let reconciliation = db
            .apply_static_config(
                &networks,
                std::slice::from_ref(&relayer),
                version(1, 1),
            )
            .await?;

        assert_eq!(reconciliation, Reconciliation::AlreadyApplied);
        let stored =
            db.get_relayer(&relayer.id).await?.context("No relayer")?;
        assert_eq!(stored.name, "changed by hand");

        Ok(())
    }

    #[tokio::test]
    async fn static_config_without_revision_forgets_the_stored_one(
    ) -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let networks = [network_config(1)];
        let relayers = [relayer_config(1, 1)];

        db.apply_static_config(&networks, &relayers, version(5, 1))
            .await?;
        db.apply_static_config(&networks, &relayers, None).await?;
        assert_eq!(db.get_static_config_version().await?, None);

        let reconciliation = db
            .apply_static_config(&networks, &relayers, version(1, 1))
            .await?;
        assert_eq!(reconciliation, Reconciliation::Applied(vec![]));
        assert_eq!(db.get_static_config_version().await?, version(1, 1));

        Ok(())
    }

    #[tokio::test]
    async fn static_config_disables_relayers_missing_from_it_and_revokes_their_keys(
    ) -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let networks = [network_config(1)];
        let kept = relayer_config(1, 1);
        let removed = relayer_config(1, 2);
        let removed_key = removed.api_key_hashes[0].0;

        db.apply_static_config(
            &networks,
            &[kept.clone(), removed.clone()],
            None,
        )
        .await?;
        db.apply_static_config(&networks, std::slice::from_ref(&kept), None)
            .await?;

        let stored =
            db.get_relayer(&removed.id).await?.context("No relayer")?;
        assert!(!stored.enabled);
        let stored = db.get_relayer(&kept.id).await?.context("No relayer")?;
        assert!(stored.enabled);
        assert_eq!(
            db.get_api_key_status(&removed.id, removed_key).await?,
            ApiKeyStatus::Unknown,
            "A removed relayer kept its API keys"
        );

        db.apply_static_config(&networks, &[kept, removed.clone()], None)
            .await?;
        let stored =
            db.get_relayer(&removed.id).await?.context("No relayer")?;
        assert!(stored.enabled);
        assert_eq!(
            db.get_api_key_status(&removed.id, removed_key).await?,
            ApiKeyStatus::Valid
        );

        Ok(())
    }

    #[tokio::test]
    async fn static_config_replaces_api_keys() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let networks = [network_config(1)];
        let relayer = RelayerConfig {
            api_key_hashes: vec![
                B256::with_last_byte(1),
                B256::with_last_byte(2),
            ],
            ..relayer_config(1, 1)
        };

        db.apply_static_config(&networks, std::slice::from_ref(&relayer), None)
            .await?;
        let rotated = RelayerConfig {
            api_key_hashes: vec![
                B256::with_last_byte(2),
                B256::with_last_byte(3),
            ],
            ..relayer.clone()
        };
        db.apply_static_config(&networks, &[rotated], None).await?;

        let status = |byte| {
            db.get_api_key_status(&relayer.id, B256::with_last_byte(byte).0)
        };
        assert_eq!(
            status(1).await?,
            ApiKeyStatus::Unknown,
            "A removed API key still works"
        );
        assert_eq!(status(2).await?, ApiKeyStatus::Valid);
        assert_eq!(status(3).await?, ApiKeyStatus::Valid);

        Ok(())
    }

    #[tokio::test]
    async fn disabled_relayers_reject_their_api_keys_and_send_nothing(
    ) -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let networks = [network_config(1)];
        let relayer = relayer_config(1, 1);

        db.apply_static_config(&networks, std::slice::from_ref(&relayer), None)
            .await?;
        create_unsent_tx(&db, &relayer.id).await?;
        assert_eq!(db.get_unsent_txs().await?.len(), 1);
        assert_eq!(db.get_disabled_relayer_backlogs().await?, vec![]);

        let disabled = RelayerConfig {
            enabled: false,
            ..relayer.clone()
        };
        db.apply_static_config(&networks, &[disabled], None).await?;

        assert_eq!(
            db.get_api_key_status(&relayer.id, relayer.api_key_hashes[0].0)
                .await?,
            ApiKeyStatus::RelayerDisabled
        );
        assert_eq!(db.get_unsent_txs().await?.len(), 0);
        assert_eq!(
            db.get_disabled_relayer_backlogs().await?,
            vec![RelayerBacklog {
                relayer_id: relayer.id,
                relayer_name: relayer.name,
                unsent_txs: 1,
                inflight_txs: 0,
            }]
        );

        Ok(())
    }

    #[tokio::test]
    async fn static_config_changes_the_address_of_a_relayer_without_history(
    ) -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let networks = [network_config(1), network_config(2)];
        let relayer = RelayerConfig {
            enabled: false,
            ..relayer_config(1, 1)
        };
        db.apply_static_config(&networks, std::slice::from_ref(&relayer), None)
            .await?;

        let fixed = RelayerConfig {
            chain_id: 2,
            address: Address::with_last_byte(9),
            enabled: true,
            ..relayer.clone()
        };
        db.apply_static_config(&networks, &[fixed], None).await?;

        let stored =
            db.get_relayer(&relayer.id).await?.context("No relayer")?;
        assert_eq!(stored.chain_id, 2);
        assert_eq!(stored.address.0, Address::with_last_byte(9));

        Ok(())
    }

    #[tokio::test]
    async fn static_config_cant_change_the_chain_or_address_of_a_relayer_with_history(
    ) -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let networks = [network_config(1), network_config(2)];
        let relayer = relayer_config(1, 1);
        db.apply_static_config(&networks, std::slice::from_ref(&relayer), None)
            .await?;
        create_unsent_tx(&db, &relayer.id).await?;

        let changes = [
            RelayerConfig {
                chain_id: 2,
                name: "renamed".to_string(),
                ..relayer.clone()
            },
            RelayerConfig {
                address: Address::with_last_byte(9),
                name: "renamed".to_string(),
                ..relayer.clone()
            },
        ];
        for changed in changes {
            let err = db
                .apply_static_config(&networks, &[changed], None)
                .await
                .unwrap_err();

            assert!(
                err.to_string().contains(&format!(
                    "relayer {} has chain id 1 and address {} in the database",
                    relayer.id,
                    relayer.address.to_string().to_lowercase()
                )),
                "{err}"
            );
        }

        let stored =
            db.get_relayer(&relayer.id).await?.context("No relayer")?;
        assert_eq!(stored.name, relayer.name, "A rejected config was applied");

        Ok(())
    }

    #[tokio::test]
    async fn static_config_keeps_one_nonce_owner_per_account(
    ) -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let networks = [network_config(1)];
        let old = relayer_config(1, 1);
        let new = RelayerConfig {
            address: old.address,
            ..relayer_config(1, 2)
        };
        db.apply_static_config(&networks, std::slice::from_ref(&old), None)
            .await?;
        create_unsent_tx(&db, &old.id).await?;

        let err = db
            .apply_static_config(&networks, std::slice::from_ref(&new), None)
            .await
            .unwrap_err();

        assert!(
            err.to_string().contains(&format!(
                "would allocate the same nonces, because they have the same \
                 chain id and address and the second one is enabled or has \
                 unmined transactions: {} and {}",
                new.id, old.id
            )),
            "{err}"
        );
        assert!(db.get_relayer(&new.id).await?.is_none());

        db.purge_unsent_txs(&old.id).await?;
        db.apply_static_config(&networks, std::slice::from_ref(&new), None)
            .await?;
        let stored = db.get_relayer(&old.id).await?.context("No relayer")?;
        assert!(!stored.enabled);

        Ok(())
    }

    #[tokio::test]
    async fn static_config_without_relayers_cant_disable_all_relayers(
    ) -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;
        let networks = [network_config(1)];
        let relayer = relayer_config(1, 1);

        db.apply_static_config(&[], &[], None).await?;
        db.apply_static_config(&networks, std::slice::from_ref(&relayer), None)
            .await?;

        let err = db.apply_static_config(&[], &[], None).await.unwrap_err();
        assert!(
            err.to_string().contains(
                "The config defines no relayers, but the database has 1 enabled relayers"
            ),
            "{err}"
        );
        let stored =
            db.get_relayer(&relayer.id).await?.context("No relayer")?;
        assert!(stored.enabled);

        let disabled_relayer = RelayerConfig {
            enabled: false,
            ..relayer
        };
        db.apply_static_config(&networks, &[disabled_relayer], None)
            .await?;
        db.apply_static_config(&[], &[], None).await?;

        Ok(())
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
