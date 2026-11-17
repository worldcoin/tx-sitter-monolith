use std::borrow::Cow;

use alloy_primitives::U256;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseError, ParseFromJSON, ToJSON};
use serde::{Deserialize, Serialize};

use crate::hex::hex_digits;

/// Maximum number of hex digits of a [`U256`], leading zeros included.
const MAX_DIGITS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexU256(pub U256);

impl Serialize for HexU256 {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let s = format!("{:#x}", self.0);
        serializer.serialize_str(&s)
    }
}

impl<'de> Deserialize<'de> for HexU256 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: Cow<str> = serde::Deserialize::deserialize(deserializer)?;

        let digits = hex_digits(&s).map_err(serde::de::Error::custom)?;

        if digits.is_empty() || digits.len() > MAX_DIGITS {
            return Err(serde::de::Error::invalid_length(
                digits.len(),
                &"between 1 and 64 hex digits",
            ));
        }

        let u256 = U256::from_str_radix(digits, 16)
            .map_err(serde::de::Error::custom)?;

        Ok(Self(u256))
    }
}

impl From<U256> for HexU256 {
    fn from(value: U256) -> Self {
        Self(value)
    }
}

impl poem_openapi::types::Type for HexU256 {
    const IS_REQUIRED: bool = true;

    type RawValueType = Self;

    type RawElementValueType = Self;

    fn name() -> std::borrow::Cow<'static, str> {
        "string(hex-u256)".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        let mut schema_ref = MetaSchema::new_with_format("string", "hex-u256");

        schema_ref.example =
            Some(serde_json::Value::String("0xff".to_string()));
        schema_ref.default = Some(serde_json::Value::String("0x0".to_string()));
        schema_ref.title = Some("Hex U256".to_string());
        schema_ref.description = Some("A hex encoded 256-bit unsigned integer");

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

impl ParseFromJSON for HexU256 {
    fn parse_from_json(
        value: Option<serde_json::Value>,
    ) -> poem_openapi::types::ParseResult<Self> {
        let value = value.ok_or_else(ParseError::expected_input)?;

        let value =
            serde_json::from_value(value).map_err(ParseError::custom)?;

        Ok(value)
    }
}

impl ToJSON for HexU256 {
    fn to_json(&self) -> Option<serde_json::Value> {
        serde_json::to_value(self).ok()
    }
}
