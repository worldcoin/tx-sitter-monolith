use std::time::Duration;

use ethers::types::{Address, H256, U256};
use sqlx::migrate::{MigrateDatabase, Migrator};
use sqlx::{Pool, Postgres, Row};

use crate::broadcast_utils::gas_estimation::FeesEstimate;
use crate::config::DatabaseConfig;

pub mod data;

use self::data::{AddressWrapper, ReadTxData};
pub use self::data::{BlockTxStatus, TxForEscalation, UnsentTx};

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
            SELECT t.id, t.tx_to, t.data, t.value, t.gas_limit, t.nonce, r.key_id, r.chain_id
            FROM transactions t
            LEFT JOIN sent_transactions s ON (t.id = s.tx_id)
            INNER JOIN relayers r ON (t.relayer_id = r.id)
            WHERE s.tx_id IS NULL
            AND (t.nonce - r.current_nonce < $1);
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
            INSERT INTO sent_transactions (tx_id, initial_max_fee_per_gas, initial_max_priority_fee_per_gas)
            VALUES ($1, $2, $3)
            "#
        )
        .bind(tx_id)
        .bind(initial_max_fee_per_gas_bytes)
        .bind(initial_max_priority_fee_per_gas_bytes)
        .execute(tx.as_mut()).await?;

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
            AND      status = $2
            AND      fee_estimate IS NOT NULL
            ORDER BY block_number DESC
            LIMIT    1
            "#,
        )
        .bind(chain_id as i64)
        .bind(BlockTxStatus::Mined)
        .fetch_optional(&self.pool)
        .await?;

        let item = row
            .map(|row| row.try_get::<sqlx::types::Json<FeesEstimate>, _>(0))
            .transpose()?;

        Ok(item.map(|json_fee_estimate| json_fee_estimate.0))
    }

    pub async fn get_next_block_numbers(
        &self,
    ) -> eyre::Result<Vec<(u64, u64)>> {
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            r#"
            SELECT   MAX(block_number) + 1, chain_id
            FROM     blocks
            WHERE    status = $1
            GROUP BY chain_id
            "#,
        )
        .bind(BlockTxStatus::Mined)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(block_number, chain_id)| {
                (block_number as u64, chain_id as u64)
            })
            .collect())
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
        txs: &[H256],
        fee_estimates: Option<&FeesEstimate>,
        status: BlockTxStatus,
    ) -> eyre::Result<()> {
        let mut db_tx = self.pool.begin().await?;

        // let fee_estimates = serde_json::to_string(&fee_estimates)?;

        let (block_id,): (i64,) = sqlx::query_as(
            r#"
            INSERT INTO blocks (block_number, chain_id, fee_estimate, status)
            VALUES      ($1, $2, $3, $4)
            RETURNING   id
            "#,
        )
        .bind(block_number as i64)
        .bind(chain_id as i64)
        .bind(fee_estimates.map(sqlx::types::Json))
        .bind(status)
        .fetch_one(db_tx.as_mut())
        .await?;

        for tx_hash in txs {
            sqlx::query(
                r#"
                INSERT INTO block_txs (block_id, tx_hash)
                VALUES ($1, $2)
                "#,
            )
            .bind(block_id)
            .bind(tx_hash.as_bytes())
            .execute(db_tx.as_mut())
            .await?;
        }

        db_tx.commit().await?;

        Ok(())
    }

    pub async fn update_transactions(
        &self,
        status: BlockTxStatus,
    ) -> eyre::Result<()> {
        sqlx::query(
            r#"
            UPDATE tx_hashes h
            SET    status = $1
            FROM   transactions t, block_txs bt, blocks b, relayers r
            WHERE  t.id = h.tx_id
            AND    b.id = bt.block_id
            AND    h.tx_hash = bt.tx_hash
            AND    r.chain_id = b.chain_id
            AND    r.id = t.relayer_id
            AND    h.status = $2
            AND    b.status = $1
            "#,
        )
        .bind(status)
        .bind(status.previous())
        .execute(&self.pool)
        .await?;

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
            AND    h.status = $2
            AND    NOT h.escalated
            "#,
        )
        .bind(escalation_interval)
        .bind(BlockTxStatus::Pending)
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

        sqlx::query(
            r#"
            UPDATE sent_transactions
            SET    escalation_count = escalation_count + 1
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
                    h.tx_hash, h.status
            FROM transactions t
            LEFT JOIN tx_hashes h ON t.id = h.tx_id
            WHERE t.id = $1
            ORDER BY h.created_at DESC, h.status DESC
            LIMIT 1
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
}

#[cfg(test)]
mod tests {
    use postgres_docker_utils::DockerContainerGuard;

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

    #[tokio::test]
    async fn basic() -> eyre::Result<()> {
        let (_db, _db_container) = setup_db().await?;

        // db.create_relayer().await?;

        Ok(())
    }
}
