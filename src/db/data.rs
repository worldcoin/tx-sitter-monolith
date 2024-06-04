use ethers::types::{H256, U256};
use serde::{Deserialize, Serialize};
use sqlx::database::{HasArguments, HasValueRef};
use sqlx::postgres::{PgHasArrayType, PgTypeInfo};
use sqlx::prelude::FromRow;
use sqlx::Database;

use crate::broadcast_utils::gas_estimation::FeesEstimate;
use crate::types::wrappers::address::AddressWrapper;
use crate::types::wrappers::hex_u256::HexU256;
use crate::types::TransactionPriority;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H256Wrapper(pub H256);

impl<'r, DB> sqlx::Decode<'r, DB> for H256Wrapper
where
    DB: Database,
    [u8; 32]: sqlx::Decode<'r, DB>,
{
    fn decode(
        value: <DB as HasValueRef<'r>>::ValueRef,
    ) -> Result<Self, sqlx::error::BoxDynError> {
        let bytes = <[u8; 32] as sqlx::Decode<DB>>::decode(value)?;

        let value = H256::from_slice(&bytes);

        Ok(Self(value))
    }
}

impl<'q, DB> sqlx::Encode<'q, DB> for H256Wrapper
where
    DB: Database,
    [u8; 32]: sqlx::Encode<'q, DB>,
{
    fn encode_by_ref(
        &self,
        buf: &mut <DB as HasArguments<'q>>::ArgumentBuffer,
    ) -> sqlx::encode::IsNull {
        <[u8; 32] as sqlx::Encode<DB>>::encode_by_ref(&self.0 .0, buf)
    }
}

impl PgHasArrayType for H256Wrapper {
    fn array_type_info() -> PgTypeInfo {
        <[u8; 32] as PgHasArrayType>::array_type_info()
    }
}

impl<DB: Database> sqlx::Type<DB> for H256Wrapper
where
    [u8; 32]: sqlx::Type<DB>,
{
    fn type_info() -> DB::TypeInfo {
        <[u8; 32] as sqlx::Type<DB>>::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        *ty == Self::type_info()
    }
}

#[derive(
    Debug, Clone, Serialize, Deserialize, Copy, PartialEq, Eq, sqlx::Type,
)]
#[sqlx(rename_all = "camelCase")]
#[sqlx(type_name = "tx_status")]
#[serde(rename_all = "camelCase")]
pub enum TxStatus {
    Pending,
    Mined,
    Finalized,
}

impl TxStatus {
    pub fn previous(self) -> Self {
        match self {
            Self::Pending => Self::Pending,
            Self::Mined => Self::Pending,
            Self::Finalized => Self::Mined,
        }
    }
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
