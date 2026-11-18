use alloy::primitives::U256;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;

use crate::broadcast_utils::gas_estimation::FeesEstimate;
use crate::db::wrappers::address::AddressWrapper;
use crate::db::wrappers::decimal_u256::DecimalU256Wrapper;
use crate::db::wrappers::h256::H256Wrapper;
use crate::db::wrappers::hex_u256::HexU256Wrapper;
use crate::tx_events::EventCursor;
use crate::types::{TransactionPriority, TxMetadata, TxStatus};

#[derive(Debug, Clone, FromRow)]
pub struct UnsentTx {
    pub relayer_id: String,
    pub id: String,
    pub tx_to: AddressWrapper,
    pub data: Vec<u8>,
    pub value: HexU256Wrapper,
    pub gas_limit: HexU256Wrapper,
    pub priority: TransactionPriority,
    #[sqlx(try_from = "i64")]
    pub nonce: u64,
    pub blobs: Option<Vec<Vec<u8>>>,
    pub key_id: String,
    #[sqlx(try_from = "i64")]
    pub chain_id: u64,
}

/// A relayer's count of pending transactions whose fees are no longer bumped,
/// see [`Database::count_txs_at_max_escalations`](crate::db::Database::count_txs_at_max_escalations).
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct RelayerTxsAtMaxEscalations {
    pub relayer_id: String,
    #[sqlx(try_from = "i64")]
    pub chain_id: u64,
    #[sqlx(try_from = "i64")]
    pub txs: u64,
}

#[derive(Debug, Clone, FromRow)]
pub struct TxForEscalation {
    pub relayer_id: String,
    pub id: String,
    pub tx_to: AddressWrapper,
    pub data: Vec<u8>,
    pub value: HexU256Wrapper,
    pub gas_limit: HexU256Wrapper,
    #[sqlx(try_from = "i64")]
    pub nonce: u64,
    pub blobs: Option<Vec<Vec<u8>>>,
    pub key_id: String,
    #[sqlx(try_from = "i64")]
    pub chain_id: u64,
    pub initial_max_fee_per_gas: HexU256Wrapper,
    pub initial_max_priority_fee_per_gas: HexU256Wrapper,
    #[sqlx(try_from = "i64")]
    pub escalation_count: usize,
    /// The bytes of the latest signed version, `None` if it was stored
    /// before signed bytes were kept
    pub raw_signed_tx: Option<Vec<u8>>,
    /// Milliseconds since the latest signed version was stored
    #[sqlx(try_from = "i64")]
    pub latest_hash_age_ms: u64,
}

/// A relayer's count of [stale transactions](crate::db::Database::count_stale_txs).
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct RelayerStaleTxs {
    pub relayer_id: String,
    #[sqlx(try_from = "i64")]
    pub chain_id: u64,
    #[sqlx(try_from = "i64")]
    pub current_nonce: u64,
    #[sqlx(try_from = "i64")]
    pub stale_txs: u64,
}

#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct ReadTxData {
    pub tx_id: String,
    pub relayer_id: String,
    #[sqlx(try_from = "i64")]
    pub chain_id: u64,
    pub to: AddressWrapper,
    pub data: Vec<u8>,
    pub value: HexU256Wrapper,
    pub gas_limit: HexU256Wrapper,
    #[sqlx(try_from = "i64")]
    pub nonce: u64,
    pub priority: TransactionPriority,
    pub blobs: Option<Vec<Vec<u8>>>,
    /// The caller's metadata, which only v2 sends store
    pub metadata: Option<TxMetadata>,
    pub created_at: DateTime<Utc>,

    // Sent tx data
    pub tx_hash: Option<H256Wrapper>,
    pub status: Option<TxStatus>,
    /// Timestamp of the block the transaction was mined in
    pub mined_at: Option<DateTime<Utc>>,
}

/// A status change of a transaction, see
/// [`Database::read_tx_events`](crate::db::Database::read_tx_events).
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct TxEventData {
    /// The event's position in the feed
    #[sqlx(flatten)]
    pub cursor: EventCursor,
    pub tx_id: String,
    pub relayer_id: String,
    #[sqlx(try_from = "i64")]
    pub chain_id: u64,
    /// The status the transaction changed to
    pub status: TxStatus,
    /// The transaction's valid hash after the change
    pub tx_hash: Option<H256Wrapper>,
    /// The caller's metadata, `None` for v1 sends and once the transaction
    /// is pruned
    pub metadata: Option<TxMetadata>,
    pub created_at: DateTime<Utc>,
}

/// How far the event feed is behind the events stored.
#[derive(Debug, Clone, FromRow, PartialEq)]
pub struct TxEventsLag {
    /// How long the first event the feed holds back has waited, `0` when it
    /// holds back none. A database transaction that writes and stays open
    /// holds back every event stored after its first write.
    pub held_back_seconds: f64,
    /// The fraction of Postgres' notification queue in use. Storing events
    /// fails once it is full, which a listener that stops reading causes.
    pub notification_queue_usage: f64,
}

#[derive(Debug, Clone)]
pub struct NetworkStats {
    pub pending_txs: u64,
    pub mined_txs: u64,
    pub finalized_txs: u64,
    pub total_indexed_blocks: u64,
    pub block_txs: u64,
}

#[derive(Debug, Clone)]
pub struct BlockFees {
    pub fee_estimates: FeesEstimate,
    pub gas_price: U256,
}

#[derive(
    Debug, Clone, Serialize, Deserialize, Copy, PartialEq, Eq, sqlx::Type,
)]
#[sqlx(rename_all = "camelCase")]
#[sqlx(type_name = "rpc_kind")]
#[serde(rename_all = "camelCase")]
pub enum RpcKind {
    Http,
    Ws,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, FromRow)]
pub struct NetworkInfo {
    #[sqlx(try_from = "i64")]
    pub chain_id: u64,
    pub name: String,
    pub http_rpc: String,
    pub ws_rpc: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayerGasPriceLimit {
    pub value: DecimalU256Wrapper,
    pub chain_id: i64,
}

/// A change [`Database::apply_static_config`] made to the database
///
/// [`Database::apply_static_config`]: crate::db::Database::apply_static_config
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigChange {
    NetworkAdded {
        chain_id: u64,
    },
    /// The network's `fields`, such as its name or RPC URLs, changed
    NetworkChanged {
        chain_id: u64,
        fields: Vec<&'static str>,
    },
    RelayerAdded {
        relayer_id: String,
        relayer_name: String,
    },
    /// The relayer's `fields`, other than `enabled`, changed
    RelayerChanged {
        relayer_id: String,
        relayer_name: String,
        fields: Vec<&'static str>,
    },
    RelayerEnabled {
        relayer_id: String,
        relayer_name: String,
    },
    /// The config disabled the relayer
    RelayerDisabled {
        relayer_id: String,
        relayer_name: String,
    },
    /// The relayer was disabled because the config no longer has it
    RelayerRemoved {
        relayer_id: String,
        relayer_name: String,
    },
    ApiKeysAdded {
        relayer_id: String,
        count: usize,
    },
    ApiKeysRevoked {
        relayer_id: String,
        count: usize,
    },
}

/// What [`Database::apply_static_config`] did
///
/// [`Database::apply_static_config`]: crate::db::Database::apply_static_config
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconciliation {
    /// The database already held the config's version, so nothing was
    /// written
    AlreadyApplied,
    /// The config was written, making these changes
    Applied(Vec<ConfigChange>),
}

/// Whether a relayer accepts an API key
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyStatus {
    /// The relayer doesn't exist or doesn't have the key
    Unknown,
    /// The relayer has the key, but is disabled
    RelayerDisabled,
    Valid,
}

/// The transactions a disabled relayer neither sent nor saw mined
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct RelayerBacklog {
    pub relayer_id: String,
    pub relayer_name: String,
    /// Transactions it never broadcast
    pub unsent_txs: i64,
    /// Broadcast transactions that aren't mined yet
    pub inflight_txs: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct RelayerInfo {
    pub id: String,
    pub name: String,
    #[sqlx(try_from = "i64")]
    pub chain_id: u64,
    pub key_id: String,
    pub address: AddressWrapper,
    #[sqlx(try_from = "i64")]
    pub nonce: u64,
    #[sqlx(try_from = "i64")]
    pub current_nonce: u64,
    #[sqlx(try_from = "i64")]
    pub max_inflight_txs: u64,
    #[sqlx(try_from = "i64")]
    pub max_queued_txs: u64,
    #[sqlx(json)]
    pub gas_price_limits: Vec<RelayerGasPriceLimit>,
    pub enabled: bool,
}

/// A relayer's nonces, which indexing syncs from its chain
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct RelayerNonces {
    pub id: String,
    pub address: AddressWrapper,
    /// The nonce the relayer gives its next transaction
    #[sqlx(try_from = "i64")]
    pub nonce: u64,
    /// The relayer's transaction count on its chain when it was last synced
    #[sqlx(try_from = "i64")]
    pub current_nonce: u64,
    /// Whether its nonce was synced from its chain since it was last
    /// enabled or moved to another account. Until then the relayer allocates
    /// no nonces.
    pub is_nonce_synced: bool,
    pub enabled: bool,
}

impl RelayerNonces {
    /// Whether the next sync is the relayer's first, which only an enabled
    /// relayer gets, see [`Database::update_relayer_nonce`].
    ///
    /// [`Database::update_relayer_nonce`]: crate::db::Database::update_relayer_nonce
    pub fn awaits_first_sync(&self) -> bool {
        self.enabled && !self.is_nonce_synced
    }
}

/// An account's transaction counts on its chain, which
/// [`Database::update_relayer_nonce`] records.
///
/// [`Database::update_relayer_nonce`]: crate::db::Database::update_relayer_nonce
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainNonces {
    /// Its transactions in the indexed block and before
    pub latest: u64,
    /// Its transactions including the ones pending in the RPC's mempool,
    /// which only a relayer's first sync needs
    pub pending: Option<u64>,
}

/// What [`Database::update_relayer_nonce`] found for the relayers of an
/// account
///
/// [`Database::update_relayer_nonce`]: crate::db::Database::update_relayer_nonce
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NonceSync {
    /// Synced relayers whose local nonce was behind the chain's, with that
    /// local nonce. Their key sent transactions that tx-sitter didn't
    /// create.
    pub behind_chain: Vec<(String, u64)>,
    /// Relayers synced for the first time
    pub first_synced: Vec<String>,
    /// Relayers whose first sync was refused, because the account has
    /// pending transactions at nonces tx-sitter never allocated
    pub pending_elsewhere: Vec<String>,
}
