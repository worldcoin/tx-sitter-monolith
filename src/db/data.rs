use ethers::types::U256;
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;

use crate::broadcast_utils::gas_estimation::FeesEstimate;
use crate::types::wrappers::address::AddressWrapper;
use crate::types::wrappers::h256::H256Wrapper;
use crate::types::wrappers::hex_u256::HexU256;
use crate::types::{TransactionPriority, TxStatus};

#[derive(Debug, Clone, FromRow)]
pub struct UnsentTx {
    pub relayer_id: String,
    pub id: String,
    pub tx_to: AddressWrapper,
    pub data: Vec<u8>,
    pub value: HexU256,
    pub gas_limit: HexU256,
    pub priority: TransactionPriority,
    #[sqlx(try_from = "i64")]
    pub nonce: u64,
    pub blobs: Option<Vec<Vec<u8>>>,
    pub key_id: String,
    #[sqlx(try_from = "i64")]
    pub chain_id: u64,
}

#[derive(Debug, Clone, FromRow)]
pub struct TxForEscalation {
    pub relayer_id: String,
    pub id: String,
    pub tx_to: AddressWrapper,
    pub data: Vec<u8>,
    pub value: HexU256,
    pub gas_limit: HexU256,
    #[sqlx(try_from = "i64")]
    pub nonce: u64,
    pub blobs: Option<Vec<Vec<u8>>>,
    pub key_id: String,
    #[sqlx(try_from = "i64")]
    pub chain_id: u64,
    pub initial_max_fee_per_gas: HexU256,
    pub initial_max_priority_fee_per_gas: HexU256,
    #[sqlx(try_from = "i64")]
    pub escalation_count: usize,
}

#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct ReadTxData {
    pub tx_id: String,
    pub to: AddressWrapper,
    pub data: Vec<u8>,
    pub value: HexU256,
    pub gas_limit: HexU256,
    #[sqlx(try_from = "i64")]
    pub nonce: u64,
    pub blobs: Option<Vec<Vec<u8>>>,

    // Sent tx data
    pub tx_hash: Option<H256Wrapper>,
    pub status: Option<TxStatus>,
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
