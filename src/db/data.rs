use ethers::types::{Address, H256, U256};
use serde::{Deserialize, Serialize};
use sqlx::database::HasValueRef;
use sqlx::prelude::FromRow;
use sqlx::Database;

#[derive(Debug, Clone, FromRow)]
pub struct UnsentTx {
    pub id: String,
    pub tx_to: AddressWrapper,
    pub data: Vec<u8>,
    pub value: U256Wrapper,
    pub gas_limit: U256Wrapper,
    #[sqlx(try_from = "i64")]
    pub nonce: u64,
    pub key_id: String,
    #[sqlx(try_from = "i64")]
    pub chain_id: u64,
}

#[derive(Debug, Clone, FromRow)]
pub struct TxForEscalation {
    pub id: String,
    pub tx_to: AddressWrapper,
    pub data: Vec<u8>,
    pub value: U256Wrapper,
    pub gas_limit: U256Wrapper,
    #[sqlx(try_from = "i64")]
    pub nonce: u64,
    pub key_id: String,
    #[sqlx(try_from = "i64")]
    pub chain_id: u64,
    pub initial_max_fee_per_gas: U256Wrapper,
    pub initial_max_priority_fee_per_gas: U256Wrapper,
    #[sqlx(try_from = "i64")]
    pub escalation_count: usize,
}

#[derive(Debug, Clone, FromRow)]
pub struct ReadTxData {
    pub tx_id: String,
    pub to: AddressWrapper,
    pub data: Vec<u8>,
    pub value: U256Wrapper,
    pub gas_limit: U256Wrapper,
    #[sqlx(try_from = "i64")]
    pub nonce: u64,

    // Sent tx data
    pub tx_hash: Option<H256Wrapper>,
    pub status: Option<BlockTxStatus>,
}

#[derive(Debug, Clone)]
pub struct AddressWrapper(pub Address);
#[derive(Debug, Clone)]
pub struct U256Wrapper(pub U256);

#[derive(Debug, Clone)]
pub struct H256Wrapper(pub H256);

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
#[sqlx(type_name = "block_tx_status")]
#[serde(rename_all = "camelCase")]
pub enum BlockTxStatus {
    Pending = 0,
    Mined = 1,
    Finalized = 2,
}

impl BlockTxStatus {
    pub fn previous(self) -> Self {
        match self {
            Self::Pending => Self::Pending,
            Self::Mined => Self::Pending,
            Self::Finalized => Self::Mined,
        }
    }
}
