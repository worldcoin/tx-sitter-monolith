use std::borrow::Cow;

use base_api_types::DecimalU256;
use ethers::types::U256;
use serde::{Deserialize, Serialize};
use sqlx::database::{HasArguments, HasValueRef};
use sqlx::Database;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DecimalU256Wrapper(pub U256);

impl Serialize for DecimalU256Wrapper {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let s = self.0.to_string();
        serializer.serialize_str(&s)
    }
}

impl<'de> Deserialize<'de> for DecimalU256Wrapper {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: Cow<str> = serde::Deserialize::deserialize(deserializer)?;

        let u256 = U256::from_dec_str(&s).map_err(serde::de::Error::custom)?;

        Ok(Self(u256))
    }
}

impl<'r, DB> sqlx::Decode<'r, DB> for DecimalU256Wrapper
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

impl<DB: Database> sqlx::Type<DB> for DecimalU256Wrapper
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

impl<'q, DB> sqlx::Encode<'q, DB> for DecimalU256Wrapper
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

impl From<U256> for DecimalU256Wrapper {
    fn from(value: U256) -> Self {
        Self(value)
    }
}

impl From<DecimalU256> for DecimalU256Wrapper {
    fn from(value: DecimalU256) -> Self {
        Self(value.0)
    }
}

impl From<DecimalU256Wrapper> for DecimalU256 {
    fn from(value: DecimalU256Wrapper) -> Self {
        Self(value.0)
    }
}
