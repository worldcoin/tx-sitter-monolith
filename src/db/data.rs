use ethers::types::{Address, H256, U256};
use serde::{Deserialize, Serialize};
use sqlx::database::{HasArguments, HasValueRef};
use sqlx::encode::IsNull;
use sqlx::postgres::{PgHasArrayType, PgTypeInfo};
use sqlx::prelude::FromRow;
use sqlx::types::BigDecimal;
use sqlx::{Database, Decode, Encode, Postgres, Type};

use std::ops::Deref;
use std::str::FromStr;

use crate::broadcast_utils::gas_estimation::FeesEstimate;
use crate::types::TransactionPriority;

#[derive(Debug, Clone, FromRow)]
pub struct UnsentTx {
    pub relayer_id: String,
    pub id: String,
    pub tx_to: AddressWrapper,
    pub data: Vec<u8>,
    pub value: U256Wrapper,
    pub gas_limit: U256Wrapper,
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
    pub value: U256Wrapper,
    pub gas_limit: U256Wrapper,
    #[sqlx(try_from = "i64")]
    pub nonce: u64,
    pub blobs: Option<Vec<Vec<u8>>>,
    pub key_id: String,
    #[sqlx(try_from = "i64")]
    pub chain_id: u64,
    pub initial_max_fee_per_gas: U256Wrapper,
    pub initial_max_priority_fee_per_gas: U256Wrapper,
    pub initial_max_fee_per_blob_gas: U128Wrapper,
    #[sqlx(try_from = "i64")]
    pub escalation_count: usize,
}

#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct ReadTxData {
    pub tx_id: String,
    pub to: AddressWrapper,
    pub data: Vec<u8>,
    pub value: U256Wrapper,
    pub gas_limit: U256Wrapper,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AddressWrapper(pub Address);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct U256Wrapper(pub U256);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H256Wrapper(pub H256);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct U128Wrapper(pub u128);

impl Type<Postgres> for U128Wrapper {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <BigDecimal as Type<Postgres>>::type_info()
    }
}

impl Deref for U128Wrapper {
    type Target = u128;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'r> Decode<'r, Postgres> for U128Wrapper {
    fn decode(
        value: <Postgres as sqlx::database::HasValueRef<'r>>::ValueRef,
    ) -> Result<Self, sqlx::error::BoxDynError> {
        let big_decimal: BigDecimal = Decode::<Postgres>::decode(value)?;
        let string_repr = big_decimal.to_string();
        let u128_value = u128::from_str_radix(&string_repr, 10)?;
        Ok(U128Wrapper(u128_value))
    }
}

impl<'q> Encode<'q, Postgres> for U128Wrapper {
    fn encode_by_ref(
        &self,
        buf: &mut <Postgres as sqlx::database::HasArguments<'q>>::ArgumentBuffer,
    ) -> IsNull {
        // Convert u128 to String and then to BigDecimal
        let big_decimal = BigDecimal::from_str(&self.0.to_string()).unwrap();
        Encode::<Postgres>::encode_by_ref(&big_decimal, buf)
    }

    fn size_hint(&self) -> usize {
        let big_decimal = BigDecimal::from_str(&self.0.to_string()).unwrap();
        Encode::<Postgres>::size_hint(&big_decimal)
    }
}

impl<'r, DB> sqlx::Decode<'r, DB> for AddressWrapper
where
    DB: Database,
    Vec<u8>: sqlx::Decode<'r, DB>,
{
    fn decode(
        value: <DB as HasValueRef<'r>>::ValueRef,
    ) -> Result<Self, sqlx::error::BoxDynError> {
        let bytes = <Vec<u8> as sqlx::Decode<DB>>::decode(value)?;

        let address = Address::from_slice(&bytes);

        Ok(Self(address))
    }
}

impl<DB: Database> sqlx::Type<DB> for AddressWrapper
where
    Vec<u8>: sqlx::Type<DB>,
{
    fn type_info() -> DB::TypeInfo {
        <Vec<u8> as sqlx::Type<DB>>::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        *ty == Self::type_info()
    }
}

impl From<Address> for AddressWrapper {
    fn from(value: Address) -> Self {
        Self(value)
    }
}

impl<'r, DB> sqlx::Decode<'r, DB> for U256Wrapper
where
    DB: Database,
    [u8; 32]: sqlx::Decode<'r, DB>,
{
    fn decode(
        value: <DB as HasValueRef<'r>>::ValueRef,
    ) -> Result<Self, sqlx::error::BoxDynError> {
        let bytes = <[u8; 32] as sqlx::Decode<DB>>::decode(value)?;

        let value = U256::from_big_endian(&bytes);

        Ok(Self(value))
    }
}

impl<DB: Database> sqlx::Type<DB> for U256Wrapper
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

impl<'q, DB> sqlx::Encode<'q, DB> for U256Wrapper
where
    DB: Database,
    [u8; 32]: sqlx::Encode<'q, DB>,
{
    fn encode_by_ref(
        &self,
        buf: &mut <DB as HasArguments<'q>>::ArgumentBuffer,
    ) -> sqlx::encode::IsNull {
        let mut bytes = [0u8; 32];
        self.0.to_big_endian(&mut bytes);

        <[u8; 32] as sqlx::Encode<DB>>::encode_by_ref(&bytes, buf)
    }
}

impl From<U256> for U256Wrapper {
    fn from(value: U256) -> Self {
        Self(value)
    }
}

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
