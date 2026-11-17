use std::borrow::Cow;

use alloy_primitives::B256;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseError, ParseFromJSON, ToJSON};
use serde::{Deserialize, Serialize};

use crate::hex::hex_digits;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct H256(pub B256);

impl<'de> Deserialize<'de> for H256 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: Cow<str> = serde::Deserialize::deserialize(deserializer)?;

        let hash = hex_digits(&s)
            .map_err(serde::de::Error::custom)?
            .parse()
            .map_err(serde::de::Error::custom)?;

        Ok(Self(hash))
    }
}

impl poem_openapi::types::Type for H256 {
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

impl ParseFromJSON for H256 {
    fn parse_from_json(
        value: Option<serde_json::Value>,
    ) -> poem_openapi::types::ParseResult<Self> {
        let value = value.ok_or_else(ParseError::expected_input)?;

        let value =
            serde_json::from_value(value).map_err(ParseError::custom)?;

        Ok(value)
    }
}

impl ToJSON for H256 {
    fn to_json(&self) -> Option<serde_json::Value> {
        serde_json::to_value(self).ok()
    }
}
