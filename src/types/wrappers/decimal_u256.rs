use ethers::types::U256;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseFromJSON, ToJSON};
use serde::{Deserialize, Serialize};
use sqlx::database::{HasArguments, HasValueRef};
use sqlx::Database;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DecimalU256(pub U256);

impl Serialize for DecimalU256 {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let s = self.0.to_string();
        serializer.serialize_str(&s)
    }
}

impl<'de> Deserialize<'de> for DecimalU256 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: &str = serde::Deserialize::deserialize(deserializer)?;

        let u256 = U256::from_dec_str(s).map_err(serde::de::Error::custom)?;

        Ok(Self(u256))
    }
}

impl<'r, DB> sqlx::Decode<'r, DB> for DecimalU256
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

impl<DB: Database> sqlx::Type<DB> for DecimalU256
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

impl<'q, DB> sqlx::Encode<'q, DB> for DecimalU256
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

impl From<U256> for DecimalU256 {
    fn from(value: U256) -> Self {
        Self(value)
    }
}

impl poem_openapi::types::Type for DecimalU256 {
    const IS_REQUIRED: bool = true;

    type RawValueType = Self;

    type RawElementValueType = Self;

    fn name() -> std::borrow::Cow<'static, str> {
        "string(u256)".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        let mut schema_ref = MetaSchema::new_with_format("string", "u256");

        schema_ref.example =
            Some(serde_json::Value::String("0xff".to_string()));
        schema_ref.title = Some("Address".to_string());
        schema_ref.description = Some(
            "A 256-bit unsigned integer. Supports hex and decimal encoding",
        );

        MetaSchemaRef::Inline(Box::new(MetaSchema::new_with_format(
            "string", "u256",
        )))
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(self)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(self.as_raw_value().into_iter())
    }
}

impl ParseFromJSON for DecimalU256 {
    fn parse_from_json(
        value: Option<serde_json::Value>,
    ) -> poem_openapi::types::ParseResult<Self> {
        // TODO: Better error handling
        let value = value
            .ok_or_else(|| poem_openapi::types::ParseError::expected_input())?;

        let inner = serde_json::from_value(value)
            .map_err(|_| poem_openapi::types::ParseError::expected_input())?;

        Ok(Self(inner))
    }
}

impl ToJSON for DecimalU256 {
    fn to_json(&self) -> Option<serde_json::Value> {
        serde_json::to_value(self.0).ok()
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case("10", 10)]
    #[test_case("255", 255)]
    fn deserialize_string(s: &str, v: u64) {
        let s = format!("\"{s}\"");
        let actual: DecimalU256 = serde_json::from_str(&s).unwrap();

        assert_eq!(actual.0, U256::from(v));
    }
}
