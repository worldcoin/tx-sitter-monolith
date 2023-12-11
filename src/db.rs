#![allow(clippy::too_many_arguments)]

use std::time::Duration;

use chrono::{DateTime, Utc};
use ethers::types::{Address, H256, U256};
use sqlx::migrate::{MigrateDatabase, Migrator};
use sqlx::types::{BigDecimal, Json};
use sqlx::{Pool, Postgres, Row};

use crate::broadcast_utils::gas_estimation::FeesEstimate;
use crate::config::DatabaseConfig;
use crate::types::{RelayerInfo, RelayerUpdate, TransactionPriority};

pub mod data;

use self::data::{
    AddressWrapper, BlockFees, H256Wrapper, NetworkStats, ReadTxData, RpcKind,
};
pub use self::data::{TxForEscalation, TxStatus, UnsentTx};

// Statically link in migration files
static MIGRATOR: Migrator = sqlx::migrate!("db/migrations");

pub struct Database {
    pub pool: Pool<Postgres>,
}

impl Database {
    pub async fn new(config: &DatabaseConfig) -> eyre::Result<Self> {
        let connection_string = config.to_connection_string();

        if !Postgres::database_exists(&connection_string).await? {
            Postgres::create_database(&connection_string).await?;
        }

        let pool = Pool::connect(&connection_string).await?;

        MIGRATOR.run(&pool).await?;

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
            INSERT INTO relayers (id, name, chain_id, key_id, address)
            VALUES ($1, $2, $3, $4, $5)
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

    pub async fn update_relayer(
        &self,
        id: &str,
        update: &RelayerUpdate,
    ) -> eyre::Result<()> {
        let mut tx = self.pool.begin().await?;

        if let Some(name) = &update.relayer_name {
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

        if let Some(max_inflight_txs) = update.max_inflight_txs {
            sqlx::query(
                r#"
                UPDATE relayers
                SET    max_inflight_txs = $2
                WHERE  id = $1
                "#,
            )
            .bind(id)
            .bind(max_inflight_txs as i64)
            .execute(tx.as_mut())
            .await?;
        }

        if let Some(gas_limits) = &update.gas_limits {
            sqlx::query(
                r#"
                UPDATE relayers
                SET    gas_limits = $2
                WHERE  id = $1
                "#,
            )
            .bind(id)
            .bind(Json(gas_limits))
            .execute(tx.as_mut())
            .await?;
        }

        tx.commit().await?;

        Ok(())
    }

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
                gas_limits
            FROM relayers
            "#,
        )
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_relayer(&self, id: &str) -> eyre::Result<RelayerInfo> {
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
                gas_limits
            FROM relayers
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn create_transaction(
        &self,
        tx_id: &str,
        to: Address,
        data: &[u8],
        value: U256,
        gas_limit: U256,
        priority: TransactionPriority,
        relayer_id: &str,
    ) -> eyre::Result<()> {
        let mut tx = self.pool.begin().await?;

        let mut value_bytes = [0u8; 32];
        value.to_big_endian(&mut value_bytes);

        let mut gas_limit_bytes = [0u8; 32];
        gas_limit.to_big_endian(&mut gas_limit_bytes);

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

        sqlx::query(
            r#"
            INSERT INTO transactions (id, tx_to, data, value, gas_limit, priority, relayer_id, nonce)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
        )
        .bind(tx_id)
        .bind(to.as_bytes())
        .bind(data)
        .bind(value_bytes)
        .bind(gas_limit_bytes)
        .bind(priority)
        .bind(relayer_id)
        .bind(nonce)
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;

        Ok(())
    }

    pub async fn get_unsent_txs(&self) -> eyre::Result<Vec<UnsentTx>> {
        Ok(sqlx::query_as(
            r#"
            SELECT     r.id as relayer_id, t.id, t.tx_to, t.data, t.value, t.gas_limit, t.priority, t.nonce, r.key_id, r.chain_id
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
        let gas_price = U256::from_dec_str(&gas_price_str)?;

        Ok(Some(BlockFees {
            fee_estimates,
            gas_price,
        }))
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
    ) -> eyre::Result<()> {
        let mut db_tx = self.pool.begin().await?;

        // Prune previously inserted block
        sqlx::query(
            r#"
            DELETE
            FROM    blocks
            WHERE   block_number = $1
            AND     chain_id = $2
            "#,
        )
        .bind(block_number as i64)
        .bind(chain_id as i64)
        .execute(db_tx.as_mut())
        .await?;

        // Insert new block
        // There can be no conflict since we remove the previous one
        sqlx::query(
            r#"
            INSERT INTO blocks (block_number, chain_id, timestamp)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(block_number as i64)
        .bind(chain_id as i64)
        .bind(timestamp)
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
            "#,
        )
        .bind(&tx_ids)
        .bind(&tx_hashes)
        .execute(tx.as_mut())
        .await?;

        tx.commit().await?;

        Ok(tx_ids)
    }

    /// Returns a list of hard reorged txs
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

        Ok(tx_ids)
    }

    /// Marks txs as mined if the associated tx hash is present in a block
    ///
    /// returns the tx ids and hashes for all mined txs
    pub async fn mine_txs(
        &self,
        chain_id: u64,
    ) -> eyre::Result<Vec<(String, H256)>> {
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

    pub async fn get_txs_for_escalation(
        &self,
        escalation_interval: Duration,
    ) -> eyre::Result<Vec<TxForEscalation>> {
        Ok(sqlx::query_as(
            r#"
            SELECT r.id as relayer_id, t.id, t.tx_to, t.data, t.value, t.gas_limit, t.nonce,
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

    pub async fn read_txs(
        &self,
        relayer_id: &str,
    ) -> eyre::Result<Vec<ReadTxData>> {
        Ok(sqlx::query_as(
            r#"
            SELECT t.id as tx_id, t.tx_to as to, t.data, t.value, t.gas_limit, t.nonce,
                   h.tx_hash, s.status
            FROM transactions t
            LEFT JOIN sent_transactions s ON t.id = s.tx_id
            LEFT JOIN tx_hashes h ON s.valid_tx_hash = h.tx_hash
            WHERE t.relayer_id = $1
            "#,
        )
        .bind(relayer_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn get_relayer_addresses(
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

    pub async fn save_api_key(
        &self,
        relayer_id: &str,
        api_key_hash: [u8; 32],
    ) -> eyre::Result<()> {
        sqlx::query(
            r#"
            INSERT INTO api_keys (relayer_id, key_hash)
            VALUES ($1, $2)
            "#,
        )
        .bind(relayer_id)
        .bind(api_key_hash)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

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

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use eyre::ContextCompat;
    use postgres_docker_utils::DockerContainerGuard;

    use super::*;
    use crate::db::data::U256Wrapper;
    use crate::types::RelayerGasLimit;

    async fn setup_db() -> eyre::Result<(Database, DockerContainerGuard)> {
        let db_container = postgres_docker_utils::setup().await?;
        let db_socket_addr = db_container.address();
        let url =
            format!("postgres://postgres:postgres@{db_socket_addr}/database");

        let db = Database::new(&DatabaseConfig::connection_string(url)).await?;

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
    async fn migration() -> eyre::Result<()> {
        let (_db, _db_container) = setup_db().await?;

        Ok(())
    }

    #[tokio::test]
    async fn reset_relayer() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;

        let chain_id = 123;
        let network_name = "network_name";
        let http_rpc = "http_rpc";
        let ws_rpc = "ws_rpc";

        db.create_network(chain_id, network_name, http_rpc, ws_rpc)
            .await?;

        let relayer_id = uuid();
        let relayer_id = relayer_id.as_str();
        let relayer_name = "relayer_name";
        let key_id = "key_id";
        let address = Address::from_low_u64_be(1);

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
            H256::from_low_u64_be(1),
            H256::from_low_u64_be(2),
            H256::from_low_u64_be(3),
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

        db.create_network(chain_id, network_name, http_rpc, ws_rpc)
            .await?;

        let relayer_id = uuid();
        let relayer_id = relayer_id.as_str();

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

        let relayer = db.get_relayer(relayer_id).await?;

        assert_eq!(relayer.id, relayer_id);
        assert_eq!(relayer.name, relayer_name);
        assert_eq!(relayer.chain_id, chain_id);
        assert_eq!(relayer.key_id, key_id);
        assert_eq!(relayer.address.0, relayer_address);
        assert_eq!(relayer.nonce, 0);
        assert_eq!(relayer.current_nonce, 0);
        assert_eq!(relayer.max_inflight_txs, 5);
        assert_eq!(relayer.gas_limits.0, vec![]);

        db.update_relayer(
            relayer_id,
            &RelayerUpdate {
                relayer_name: None,
                max_inflight_txs: Some(10),
                gas_limits: Some(vec![RelayerGasLimit {
                    chain_id: 1,
                    value: U256Wrapper(U256::from(10_123u64)),
                }]),
            },
        )
        .await?;

        let relayer = db.get_relayer(relayer_id).await?;

        assert_eq!(relayer.id, relayer_id);
        assert_eq!(relayer.name, relayer_name);
        assert_eq!(relayer.chain_id, chain_id);
        assert_eq!(relayer.key_id, key_id);
        assert_eq!(relayer.address.0, relayer_address);
        assert_eq!(relayer.nonce, 0);
        assert_eq!(relayer.current_nonce, 0);
        assert_eq!(relayer.max_inflight_txs, 10);
        assert_eq!(
            relayer.gas_limits.0,
            vec![RelayerGasLimit {
                chain_id: 1,
                value: U256Wrapper(U256::from(10_123u64)),
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

        db.create_network(chain_id, network_name, http_rpc, ws_rpc)
            .await?;

        let relayer_id = uuid();
        let relayer_id = relayer_id.as_str();

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
        let priority = TransactionPriority::Regular;

        let tx = db.read_tx(tx_id).await?;
        assert!(tx.is_none(), "Tx has not been sent yet");

        db.create_transaction(
            tx_id, to, data, value, gas_limit, priority, relayer_id,
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
    async fn blocks() -> eyre::Result<()> {
        let (db, _db_container) = setup_db().await?;

        let block_number = 1;
        let chain_id = 1;
        let timestamp = ymd_hms(2023, 11, 23, 12, 32, 2);
        let txs = &[
            H256::from_low_u64_be(1),
            H256::from_low_u64_be(2),
            H256::from_low_u64_be(3),
        ];

        db.save_block(block_number, chain_id, timestamp, txs)
            .await?;

        let fee_estimates = FeesEstimate {
            base_fee_per_gas: U256::from(13_132),
            percentile_fees: vec![U256::from(0)],
        };

        let gas_price = U256::from(1_000_000_007);

        db.save_block_fees(block_number, chain_id, &fee_estimates, gas_price)
            .await?;

        let block_fees = db.get_latest_block_fees_by_chain_id(chain_id).await?;

        let block_fees = block_fees.context("Missing fees")?;

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
