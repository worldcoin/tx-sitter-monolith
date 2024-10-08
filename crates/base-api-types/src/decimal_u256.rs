use std::borrow::Cow;

use ethers::types::U256;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseError, ParseFromJSON, ToJSON};
use serde::{Deserialize, Serialize};

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
        let s: Cow<str> = serde::Deserialize::deserialize(deserializer)?;

        let u256 = U256::from_dec_str(&s).map_err(serde::de::Error::custom)?;

        Ok(Self(u256))
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

    fn name() -> Cow<'static, str> {
        "string(decimal-u256)".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        let mut schema_ref =
            MetaSchema::new_with_format("string", "decimal-u256");

        schema_ref.example = Some(serde_json::Value::String("0".to_string()));
        schema_ref.default = Some(serde_json::Value::String("0".to_string()));
        schema_ref.title = Some("Decimal U256".to_string());
        schema_ref.description = Some("A decimal 256-bit unsigned integer");

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

impl ParseFromJSON for DecimalU256 {
    fn parse_from_json(
        value: Option<serde_json::Value>,
    ) -> poem_openapi::types::ParseResult<Self> {
        let value = value.ok_or_else(ParseError::expected_input)?;

        let value =
            serde_json::from_value(value).map_err(ParseError::custom)?;

        Ok(value)
    }
}

impl ToJSON for DecimalU256 {
    fn to_json(&self) -> Option<serde_json::Value> {
        serde_json::to_value(self).ok()
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case("10", 10)]
    #[test_case("255", 255)]
    #[test_case("10000000000000000000", 10000000000000000000)]
    fn deserialize_string(s: &str, v: u64) {
        let s = format!("\"{s}\"");
        let actual: DecimalU256 = serde_json::from_str(&s).unwrap();

        assert_eq!(actual.0, U256::from(v));

        let reserialized = serde_json::to_string(&actual).unwrap();

        assert_eq!(reserialized, s);
    }
}
