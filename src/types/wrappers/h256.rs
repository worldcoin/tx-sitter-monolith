use ethers::types::H256;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseFromJSON, ToJSON};
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

impl poem_openapi::types::Type for H256Wrapper {
    const IS_REQUIRED: bool = true;

    type RawValueType = Self;

    type RawElementValueType = Self;

    fn name() -> std::borrow::Cow<'static, str> {
        "string(h256)".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        let mut schema_ref = MetaSchema::new_with_format("string", "h256");

        schema_ref.example = Some(serde_json::Value::String(
            "0x46239dbfe5502b9f82c3dff992927d8d9b3168e732b4fd5771288569f5a1813d".to_string(),
        ));
        schema_ref.default = Some(serde_json::Value::String(
            "0x0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        ));
        schema_ref.title = Some("H256".to_string());
        schema_ref.description = Some("A hex encoded 256-bit hash");

        MetaSchemaRef::Inline(Box::new(schema_ref))
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

impl ParseFromJSON for H256Wrapper {
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

impl ToJSON for H256Wrapper {
    fn to_json(&self) -> Option<serde_json::Value> {
        serde_json::to_value(self.0).ok()
    }
}