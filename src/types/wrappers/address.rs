use ethers::types::Address;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseFromJSON, ToJSON};
use serde::{Deserialize, Serialize};
use sqlx::database::HasValueRef;
use sqlx::Database;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AddressWrapper(pub Address);

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

impl poem_openapi::types::Type for AddressWrapper {
    const IS_REQUIRED: bool = true;

    type RawValueType = Address;

    type RawElementValueType = Address;

    fn name() -> std::borrow::Cow<'static, str> {
        "string(address)".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        let mut schema_ref = MetaSchema::new_with_format(
            "string", "address",
        );

        schema_ref.example = Some(serde_json::Value::String("0x000000000000000000000000000000000000000f".to_string()));
        schema_ref.title = Some("Address".to_string());
        schema_ref.description = Some("Hex encoded ethereum address");

        MetaSchemaRef::Inline(Box::new(schema_ref))
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(&self.0)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(self.as_raw_value().into_iter())
    }
}

impl ParseFromJSON for AddressWrapper {
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

impl ToJSON for AddressWrapper {
    fn to_json(&self) -> Option<serde_json::Value> {
        serde_json::to_value(self.0).ok()
    }
}
