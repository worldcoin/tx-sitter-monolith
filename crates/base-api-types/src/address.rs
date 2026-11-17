use std::borrow::Cow;

use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseError, ParseFromJSON, ToJSON};
use serde::{Deserialize, Serialize};

use crate::hex::hex_digits;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Address(pub alloy_primitives::Address);

impl<'de> Deserialize<'de> for Address {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: Cow<str> = serde::Deserialize::deserialize(deserializer)?;

        let address = hex_digits(&s)
            .map_err(serde::de::Error::custom)?
            .parse()
            .map_err(serde::de::Error::custom)?;

        Ok(Self(address))
    }
}

impl From<alloy_primitives::Address> for Address {
    fn from(value: alloy_primitives::Address) -> Self {
        Self(value)
    }
}

impl poem_openapi::types::Type for Address {
    const IS_REQUIRED: bool = true;

    type RawValueType = alloy_primitives::Address;

    type RawElementValueType = alloy_primitives::Address;

    fn name() -> std::borrow::Cow<'static, str> {
        "string(address)".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        let mut schema_ref = MetaSchema::new_with_format("string", "address");

        schema_ref.example = Some(serde_json::Value::String(
            "0x000000000000000000000000000000000000000f".to_string(),
        ));
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

impl ParseFromJSON for Address {
    fn parse_from_json(
        value: Option<serde_json::Value>,
    ) -> poem_openapi::types::ParseResult<Self> {
        let value = value.ok_or_else(ParseError::expected_input)?;

        let value =
            serde_json::from_value(value).map_err(ParseError::custom)?;

        Ok(value)
    }
}

impl ToJSON for Address {
    fn to_json(&self) -> Option<serde_json::Value> {
        serde_json::to_value(self).ok()
    }
}
