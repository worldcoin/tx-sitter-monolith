use ethers::types::H256;
use serde::{Deserialize, Serialize};
use sqlx::database::{HasArguments, HasValueRef};
use sqlx::postgres::{PgHasArrayType, PgTypeInfo};
use sqlx::Database;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
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

impl From<base_api_types::H256> for H256Wrapper {
    fn from(value: base_api_types::H256) -> Self {
        Self(value.0)
    }
}

impl From<H256Wrapper> for base_api_types::H256 {
    fn from(value: H256Wrapper) -> Self {
        Self(value.0)
    }
}
