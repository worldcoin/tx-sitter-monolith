use ethers::types::Bytes;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseError, ParseFromJSON, ToJSON};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HexBytes(pub Bytes);

impl From<Bytes> for HexBytes {
    fn from(value: Bytes) -> Self {
        Self(value)
    }
}

impl From<Vec<u8>> for HexBytes {
    fn from(value: Vec<u8>) -> Self {
        Self(Bytes::from(value))
    }
}

impl poem_openapi::types::Type for HexBytes {
    const IS_REQUIRED: bool = true;

    type RawValueType = Self;

    type RawElementValueType = Self;

    fn name() -> std::borrow::Cow<'static, str> {
        "string(bytes)".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        let mut schema_ref = MetaSchema::new_with_format("string", "bytes");

        schema_ref.example =
            Some(serde_json::Value::String("0xffffff".to_string()));
        schema_ref.title = Some("Bytes".to_string());
        schema_ref.description = Some("Hex encoded binary blob");

        MetaSchemaRef::Inline(Box::new(MetaSchema::new_with_format(
            "string", "bytes",
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

impl ParseFromJSON for HexBytes {
    fn parse_from_json(
        value: Option<serde_json::Value>,
    ) -> poem_openapi::types::ParseResult<Self> {
        let value = value.ok_or_else(ParseError::expected_input)?;

        let inner =
            serde_json::from_value(value).map_err(ParseError::custom)?;

        Ok(Self(inner))
    }
}

impl ToJSON for HexBytes {
    fn to_json(&self) -> Option<serde_json::Value> {
        serde_json::to_value(&self.0).ok()
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case("0xff", vec![255])]
    #[test_case("0xffff", vec![255, 255])]
    #[test_case("0x0101", vec![1, 1])]
    fn deserialize_string(s: &str, v: Vec<u8>) {
        let value = serde_json::Value::String(s.to_string());
        let result = HexBytes::parse_from_json(Some(value)).unwrap();
        assert_eq!(result.0, Bytes::from(v));
    }
}
