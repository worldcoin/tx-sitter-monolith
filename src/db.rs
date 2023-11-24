use std::time::Duration;

use chrono::{DateTime, Utc};
use ethers::types::{Address, H256, U256};
use sqlx::migrate::{MigrateDatabase, Migrator};
use sqlx::{Pool, Postgres, Row};

use crate::broadcast_utils::gas_estimation::FeesEstimate;
use crate::config::DatabaseConfig;

pub mod data;

use self::data::{AddressWrapper, H256Wrapper, ReadTxData, RpcKind};
pub use self::data::{TxForEscalation, TxStatus, UnsentTx};

// Statically link in migration files
static MIGRATOR: Migrator = sqlx::migrate!("db/migrations");

pub struct Database {
    pub pool: Pool<Postgres>,
}

impl Database {
    pub async fn new(config: &DatabaseConfig) -> eyre::Result<Self> {
        let pool = loop {
            if !Postgres::database_exists(&config.connection_string).await? {
                Postgres::create_database(&config.connection_string).await?;
            }

            let pool = Pool::connect(&config.connection_string).await?;

            if let Err(err) = MIGRATOR.run(&pool).await {
                tracing::error!("{err:?}");
                tracing::warn!("Migration mismatch dropping previosu db");
                drop(pool);
                // Drop the DB if it's out of date - ONLY FOR TESTING
                Postgres::drop_database(&config.connection_string).await?;
            } else {
                break pool;
            }
        };

        Ok(Self { pool })
    }

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
            INSERT INTO relayers (id, name, chain_id, key_id, address, nonce, current_nonce)
            VALUES ($1, $2, $3, $4, $5, 0, 0)
        "#,
        )
        .bind(id)
        .bind(name)
        .bind(chain_id as i64)
        .bind(key_id)
        .bind(address.as_bytes())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn create_transaction(
        &self,
        tx_id: &str,
        to: Address,
        data: &[u8],
        value: U256,
        gas_limit: U256,
        relayer_id: &str,
    ) -> eyre::Result<()> {
        let mut tx = self.pool.begin().await?;
        let mut value_bytes = [0u8; 32];
        value.to_big_endian(&mut value_bytes);

        let mut gas_limit_bytes = [0u8; 32];
        gas_limit.to_big_endian(&mut gas_limit_bytes);

        sqlx::query(
            r#"
            INSERT INTO transactions (id, tx_to, data, value, gas_limit, relayer_id, nonce)
            VALUES ($1, $2, $3, $4, $5, $6, (SELECT nonce FROM relayers WHERE id = $6))
        "#,
        )
        .bind(tx_id)
        .bind(to.as_bytes())
        .bind(data)
        .bind(value_bytes)
        .bind(gas_limit_bytes)
        .bind(relayer_id)
        .execute(tx.as_mut())
        .await?;

        sqlx::query(
            r#"
            UPDATE relayers
            SET nonce = nonce + 1,
                updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(relayer_id)
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;

        Ok(())
    }

    pub async fn get_unsent_txs(
        &self,
        max_inflight_txs: usize,
    ) -> eyre::Result<Vec<UnsentTx>> {
        Ok(sqlx::query_as(
            r#"
            SELECT     t.id, t.tx_to, t.data, t.value, t.gas_limit, t.nonce, r.key_id, r.chain_id
            FROM       transactions t
            LEFT JOIN  sent_transactions s ON (t.id = s.tx_id)
            INNER JOIN relayers r ON (t.relayer_id = r.id)
            WHERE      s.tx_id IS NULL
            AND        (t.nonce - r.current_nonce < $1);
            "#,
        )
        .bind(max_inflight_txs as i64)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn insert_tx_broadcast(
        &self,
        tx_id: &str,
        tx_hash: H256,
        initial_max_fee_per_gas: U256,
        initial_max_priority_fee_per_gas: U256,
    ) -> eyre::Result<()> {
        let mut initial_max_fee_per_gas_bytes = [0u8; 32];
        initial_max_fee_per_gas
            .to_big_endian(&mut initial_max_fee_per_gas_bytes);

        let mut initial_max_priority_fee_per_gas_bytes = [0u8; 32];
        initial_max_priority_fee_per_gas
            .to_big_endian(&mut initial_max_priority_fee_per_gas_bytes);

        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            INSERT INTO tx_hashes (tx_id, tx_hash, max_fee_per_gas, max_priority_fee_per_gas)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(tx_id)
        .bind(tx_hash.as_bytes())
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
        .bind(tx_hash.as_bytes())
        .execute(tx.as_mut()).await?;

        tx.commit().await?;

        Ok(())
    }

    pub async fn get_latest_block_fees_by_chain_id(
        &self,
        chain_id: u64,
    ) -> eyre::Result<Option<FeesEstimate>> {
        let row = sqlx::query(
            r#"
            SELECT   fee_estimate
            FROM     blocks
            WHERE    chain_id = $1
            AND      fee_estimate IS NOT NULL
            ORDER BY block_number DESC
            LIMIT    1
            "#,
        )
        .bind(chain_id as i64)
        .fetch_optional(&self.pool)
        .await?;

        let item = row
            .map(|row| row.try_get::<sqlx::types::Json<FeesEstimate>, _>(0))
            .transpose()?;

        Ok(item.map(|json_fee_estimate| json_fee_estimate.0))
    }

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

    pub async fn save_block(
        &self,
        block_number: u64,
        chain_id: u64,
        timestamp: DateTime<Utc>,
        txs: &[H256],
        fee_estimates: Option<&FeesEstimate>,
    ) -> eyre::Result<()> {
        let mut db_tx = self.pool.begin().await?;

        // Prune block txs
        sqlx::query(
            r#"
            DELETE
            FROM  block_txs
            WHERE block_number = $1
            AND   chain_id = $2
            "#,
        )
        .bind(block_number as i64)
        .bind(chain_id as i64)
        .execute(db_tx.as_mut())
        .await?;

        // Insert new block or update
        sqlx::query(
            r#"
            INSERT INTO blocks (block_number, chain_id, timestamp, fee_estimate)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (block_number, chain_id) DO UPDATE
            SET    timestamp = $3,
                   fee_estimate = $4
            "#,
        )
        .bind(block_number as i64)
        .bind(chain_id as i64)
        .bind(timestamp)
        .bind(fee_estimates.map(sqlx::types::Json))
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
            );
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

    pub async fn handle_soft_reorgs(&self) -> eyre::Result<()> {
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
            "#,
        )
        .bind(&tx_ids)
        .bind(&tx_hashes)
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;

        Ok(())
    }

    pub async fn handle_hard_reorgs(&self) -> eyre::Result<()> {
        let mut tx = self.pool.begin().await?;

        // Fetch txs which are marked as mined
        // but none of the associated tx hashes are present in block txs
        let items: Vec<(String,)> = sqlx::query_as(
            r#"
            WITH fdsa AS (
                SELECT     t.id, h.tx_hash, bt.chain_id
                FROM       transactions t
                JOIN       sent_transactions s ON t.id = s.tx_id
                JOIN       tx_hashes h ON t.id = h.tx_id
                LEFT JOIN  block_txs bt ON h.tx_hash = bt.tx_hash
                WHERE      s.status = $1
            )
            SELECT    t.id
            FROM      fdsa t
            GROUP BY  t.id
            HAVING    COUNT(t.chain_id) = 0
            "#,
        )
        .bind(TxStatus::Mined)
        .fetch_all(tx.as_mut())
        .await?;

        let tx_ids: Vec<_> = items.into_iter().map(|(x,)| x).collect();

        // Set status to pending
        // and set valid tx hash to the latest tx hash
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
            "#,
        )
        .bind(TxStatus::Pending)
        .bind(&tx_ids)
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;

        Ok(())
    }

    pub async fn mine_txs(&self, chain_id: u64) -> eyre::Result<()> {
        let mut tx = self.pool.begin().await?;

        // Fetch txs which are marked as pending but have an associated tx
        // present in in one of the block txs
        let items: Vec<(String, H256Wrapper, DateTime<Utc>)> = sqlx::query_as(
            r#"
            SELECT t.id, h.tx_hash, b.timestamp
            FROM   transactions t
            JOIN   sent_transactions s ON t.id = s.tx_id
            JOIN   tx_hashes h ON t.id = h.tx_id
            JOIN   block_txs bt ON h.tx_hash = bt.tx_hash
            JOIN   blocks b ON bt.block_number = b.block_number AND bt.chain_id = b.chain_id
            WHERE  s.status = $1
            AND    b.chain_id = $2
            "#,
        )
        .bind(TxStatus::Pending)
        .bind(chain_id as i64)
        .fetch_all(tx.as_mut())
        .await?;

        let mut tx_ids = Vec::new();
        let mut tx_hashes = Vec::new();
        let mut timestamps = Vec::new();

        for (tx_id, tx_hash, timestamp) in items {
            tx_ids.push(tx_id);
            tx_hashes.push(tx_hash);
            timestamps.push(timestamp);
        }

        sqlx::query(
            r#"
            UPDATE sent_transactions s
            SET    status = $1,
                   valid_tx_hash = mined.tx_hash,
                   mined_at = mined.timestamp
            FROM   transactions t,
                   UNNEST($2::TEXT[], $3::BYTEA[], $4::TIMESTAMPTZ[]) AS mined(tx_id, tx_hash, timestamp)
            WHERE  t.id = mined.tx_id
            AND    t.id = s.tx_id
            "#,
        )
        .bind(TxStatus::Mined)
        .bind(&tx_ids)
        .bind(&tx_hashes)
        .bind(&timestamps)
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;

        Ok(())
    }

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

        // Set status to finalized
        sqlx::query(
            r#"
            UPDATE sent_transactions s
            SET    status = $1
            FROM   transactions t, UNNEST($2::TEXT[]) AS finalized(tx_id)
            WHERE  t.id = finalized.tx_id
            AND    t.id = s.tx_id
            "#,
        )
        .bind(TxStatus::Finalized)
        .bind(&tx_ids)
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;

        Ok(())
    }

    pub async fn fetch_txs_for_escalation(
        &self,
        escalation_interval: Duration,
    ) -> eyre::Result<Vec<TxForEscalation>> {
        Ok(sqlx::query_as(
            r#"
            SELECT t.id, t.tx_to, t.data, t.value, t.gas_limit, t.nonce,
                   r.key_id, r.chain_id,
                   s.initial_max_fee_per_gas, s.initial_max_priority_fee_per_gas, s.escalation_count
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

    pub async fn escalate_tx(
        &self,
        tx_id: &str,
        tx_hash: H256,
        max_fee_per_gas: U256,
        max_priority_fee_per_gas: U256,
    ) -> eyre::Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            UPDATE tx_hashes
            SET    escalated = true
            WHERE  tx_id = $1
            "#,
        )
        .bind(tx_id)
        .execute(tx.as_mut())
        .await?;

        let mut max_fee_per_gas_bytes = [0u8; 32];
        max_fee_per_gas.to_big_endian(&mut max_fee_per_gas_bytes);

        let mut max_priority_fee_per_gas_bytes = [0u8; 32];
        max_priority_fee_per_gas
            .to_big_endian(&mut max_priority_fee_per_gas_bytes);

        sqlx::query(
            r#"
            INSERT INTO tx_hashes (tx_id, tx_hash, max_fee_per_gas, max_priority_fee_per_gas)
            VALUES ($1, $2, $3, $4)
            "#
        )
        .bind(tx_id)
        .bind(tx_hash.as_bytes())
        .bind(max_fee_per_gas_bytes)
        .bind(max_priority_fee_per_gas_bytes)
        .execute(tx.as_mut())
        .await?;

        sqlx::query(
            r#"
            UPDATE sent_transactions
            SET    escalation_count = escalation_count + 1,
                   valid_tx_hash = $2
            WHERE  tx_id = $1
            "#,
        )
        .bind(tx_id)
        .bind(tx_hash.as_bytes())
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;

        Ok(())
    }

    pub async fn read_tx(
        &self,
        tx_id: &str,
    ) -> eyre::Result<Option<ReadTxData>> {
        Ok(sqlx::query_as(
            r#"
            SELECT t.id as tx_id, t.tx_to as to, t.data, t.value, t.gas_limit, t.nonce,
                   h.tx_hash, s.status
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

    pub async fn fetch_relayer_addresses(
        &self,
        chain_id: u64,
    ) -> eyre::Result<Vec<Address>> {
        let items: Vec<(AddressWrapper,)> = sqlx::query_as(
            r#"
            SELECT address
            FROM   relayers
            WHERE  chain_id = $1
            "#,
        )
        .bind(chain_id as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(items.into_iter().map(|(wrapper,)| wrapper.0).collect())
    }

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
        .bind(relayer_address.as_bytes())
        .bind(nonce as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

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

    pub async fn create_network(
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
                ($1, $2, $3),
                ($1, $4, $5)
            "#,
        )
        .bind(chain_id as i64)
        .bind(http_rpc)
        .bind(RpcKind::Http)
        .bind(ws_rpc)
        .bind(RpcKind::Ws)
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;

        Ok(())
    }

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
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use eyre::ContextCompat;
    use postgres_docker_utils::DockerContainerGuard;
    use tracing::level_filters::LevelFilter;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::EnvFilter;

    use super::*;

    async fn setup_db() -> eyre::Result<(Database, DockerContainerGuard)> {
        let db_container = postgres_docker_utils::setup().await?;
        let db_socket_addr = db_container.address();
        let url =
            format!("postgres://postgres:postgres@{db_socket_addr}/database");

        let db = Database::new(&DatabaseConfig {
            connection_string: url,
        })
        .await?;

        Ok((db, db_container))
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
    async fn basic() -> eyre::Result<()> {
        let (_db, _db_container) = setup_db().await?;

        // db.create_relayer().await?;

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
            H256::from_low_u64_be(1),
            H256::from_low_u64_be(2),
            H256::from_low_u64_be(3),
        ];

        db.save_block(1, 1, block_timestamp, &tx_hashes, None)
            .await?;

        assert!(db.has_blocks_for_chain(1).await?, "Should have blocks");

        db.prune_blocks(prune_timestamp).await?;

        assert!(!db.has_blocks_for_chain(1).await?, "Should not have blocks");

        Ok(())
    }

    #[tokio::test]
    async fn tx_lifecycle() -> eyre::Result<()> {
        tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().pretty().compact())
            .with(
                EnvFilter::builder()
                    .with_default_directive(LevelFilter::INFO.into())
                    // Logging from fake_rpc can get very messy so we set it to warn only
                    .parse_lossy("info"),
            )
            .init();

        let (db, _db_container) = setup_db().await?;

        let chain_id = 123;
        let network_name = "network_name";
        let http_rpc = "http_rpc";
        let ws_rpc = "ws_rpc";

        db.create_network(chain_id, network_name, http_rpc, ws_rpc)
            .await?;

        let relayer_id = "relayer_id";
        let relayer_name = "relayer_name";
        let key_id = "key_id";
        let relayer_address = Address::from_low_u64_be(1);

        db.create_relayer(
            relayer_id,
            relayer_name,
            chain_id,
            key_id,
            relayer_address,
        )
        .await?;

        let tx_id = "tx_id";
        let to = Address::from_low_u64_be(1);
        let data: &[u8] = &[];
        let value = U256::from(0);
        let gas_limit = U256::from(0);

        let tx = db.read_tx(tx_id).await?;
        assert!(tx.is_none(), "Tx has not been sent yet");

        db.create_transaction(tx_id, to, data, value, gas_limit, relayer_id)
            .await?;

        let tx = db.read_tx(tx_id).await?.context("Missing tx")?;

        assert_eq!(tx.tx_id, tx_id);
        assert_eq!(tx.to.0, to);
        assert_eq!(tx.data, data);
        assert_eq!(tx.value.0, value);
        assert_eq!(tx.gas_limit.0, gas_limit);
        assert_eq!(tx.nonce, 0);
        assert_eq!(tx.tx_hash, None);

        let tx_hash_1 = H256::from_low_u64_be(1);
        let tx_hash_2 = H256::from_low_u64_be(2);
        let initial_max_fee_per_gas = U256::from(1);
        let initial_max_priority_fee_per_gas = U256::from(1);

        db.insert_tx_broadcast(
            tx_id,
            tx_hash_1,
            initial_max_fee_per_gas,
            initial_max_priority_fee_per_gas,
        )
        .await?;

        let tx = db.read_tx(tx_id).await?.context("Missing tx")?;

        assert_eq!(tx.tx_hash.unwrap().0, tx_hash_1);
        assert_eq!(tx.status, Some(TxStatus::Pending));

        db.escalate_tx(
            tx_id,
            tx_hash_2,
            initial_max_fee_per_gas,
            initial_max_priority_fee_per_gas,
        )
        .await?;

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

        db.save_block(block_number, chain_id, timestamp, txs, None)
            .await?;

        full_update(&db, chain_id, finalized_timestamp).await?;

        let tx = db.read_tx(tx_id).await?.context("Missing tx")?;

        assert_eq!(tx.tx_hash.unwrap().0, tx_hash_1);
        assert_eq!(tx.status, Some(TxStatus::Mined));

        // Reorg
        let txs = &[tx_hash_2];

        db.save_block(block_number, chain_id, timestamp, txs, None)
            .await?;

        full_update(&db, chain_id, finalized_timestamp).await?;

        let tx = db.read_tx(tx_id).await?.context("Missing tx")?;

        assert_eq!(tx.tx_hash.unwrap().0, tx_hash_2);
        assert_eq!(tx.status, Some(TxStatus::Mined));

        // Destructive reorg
        let txs = &[];

        db.save_block(block_number, chain_id, timestamp, txs, None)
            .await?;

        full_update(&db, chain_id, finalized_timestamp).await?;

        let tx = db.read_tx(tx_id).await?.context("Missing tx")?;

        assert_eq!(tx.tx_hash.unwrap().0, tx_hash_2);
        assert_eq!(tx.status, Some(TxStatus::Pending));

        // Finalization
        let txs = &[tx_hash_2];

        db.save_block(block_number, chain_id, timestamp, txs, None)
            .await?;

        let finalized_timestamp = ymd_hms(2023, 11, 23, 22, 0, 0);
        full_update(&db, chain_id, finalized_timestamp).await?;

        let tx = db.read_tx(tx_id).await?.context("Missing tx")?;

        assert_eq!(tx.tx_hash.unwrap().0, tx_hash_2);
        assert_eq!(tx.status, Some(TxStatus::Finalized));

        Ok(())
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
}
