use std::borrow::Cow;

use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseError, ParseFromJSON, ParseResult, ToJSON};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;
use serde_json::Value;
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::postgres::{PgArgumentBuffer, PgTypeInfo, PgValueRef};
use sqlx::{Decode, Encode, Postgres};

/// The largest metadata the API accepts, in bytes of JSON text
pub const MAX_TX_METADATA_SIZE: usize = 64 * 1024;

/// A JSON object the caller stores with a transaction.
///
/// tx-sitter doesn't interpret metadata. It keeps the JSON text exactly as
/// the caller sent it, whitespace and key order included, and returns that
/// text, so that numbers of any size come back unchanged. The API accepts
/// objects of at most [`MAX_TX_METADATA_SIZE`] bytes.
///
/// Read and write metadata with serde. poem-openapi converts JSON through
/// [`Value`], which holds integers of at most 64 bits, so the
/// [`ParseFromJSON`] and [`ToJSON`] impls only let request and response types
/// describe their schema.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct TxMetadata(Box<RawValue>);

impl TxMetadata {
    /// The JSON text as the caller sent it
    pub fn as_str(&self) -> &str {
        self.0.get()
    }
}

impl TryFrom<Box<RawValue>> for TxMetadata {
    type Error = eyre::Report;

    fn try_from(json: Box<RawValue>) -> Result<Self, Self::Error> {
        let size = json.get().len();
        eyre::ensure!(
            size <= MAX_TX_METADATA_SIZE,
            "metadata is {size} bytes, more than the limit of {MAX_TX_METADATA_SIZE}"
        );

        // The text is a single JSON value without surrounding whitespace
        eyre::ensure!(
            json.get().starts_with('{'),
            "metadata must be a JSON object"
        );

        Ok(Self(json))
    }
}

impl<'de> Deserialize<'de> for TxMetadata {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let json = Box::<RawValue>::deserialize(deserializer)?;

        Self::try_from(json).map_err(serde::de::Error::custom)
    }
}

/// Requests with the same metadata text are the same request
impl PartialEq for TxMetadata {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for TxMetadata {}

impl sqlx::Type<Postgres> for TxMetadata {
    fn type_info() -> PgTypeInfo {
        <str as sqlx::Type<Postgres>>::type_info()
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        <str as sqlx::Type<Postgres>>::compatible(ty)
    }
}

impl Encode<'_, Postgres> for TxMetadata {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        <&str as Encode<Postgres>>::encode(self.as_str(), buf)
    }
}

impl<'r> Decode<'r, Postgres> for TxMetadata {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        let json = <String as Decode<Postgres>>::decode(value)?;

        Ok(Self(RawValue::from_string(json)?))
    }
}

impl poem_openapi::types::Type for TxMetadata {
    const IS_REQUIRED: bool = true;

    type RawValueType = Self;

    type RawElementValueType = Self;

    fn name() -> Cow<'static, str> {
        "object(tx-metadata)".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        let mut schema_ref = MetaSchema::new("object");

        schema_ref.additional_properties =
            Some(Box::new(MetaSchemaRef::Inline(Box::new(MetaSchema::ANY))));
        schema_ref.example =
            Some(serde_json::json!({ "transactionType": "SafeDeployment" }));
        schema_ref.title = Some("Transaction metadata".to_string());
        schema_ref.description = Some(
            "A JSON object stored with the transaction and returned exactly as \
             sent, at most 64 KiB of JSON text.",
        );

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

impl ParseFromJSON for TxMetadata {
    fn parse_from_json(value: Option<Value>) -> ParseResult<Self> {
        let value = value.ok_or_else(ParseError::expected_input)?;
        let json = serde_json::value::to_raw_value(&value)
            .map_err(ParseError::custom)?;

        Self::try_from(json).map_err(ParseError::custom)
    }
}

impl ToJSON for TxMetadata {
    fn to_json(&self) -> Option<Value> {
        serde_json::from_str(self.as_str()).ok()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use test_case::test_case;

    use super::*;

    fn parse(json: &str) -> serde_json::Result<TxMetadata> {
        serde_json::from_str(json)
    }

    /// An object whose JSON text is exactly `size` bytes
    fn object_of_size(size: usize) -> String {
        let overhead = r#"{"k":""}"#.len();

        json!({ "k": "x".repeat(size - overhead) }).to_string()
    }

    #[test]
    fn keeps_the_json_text() {
        let json = r#"{ "b": 18446744073709551617, "a": [1.10, 1e2] }"#;

        let metadata = parse(&format!(" \n{json}\t")).unwrap();

        assert_eq!(metadata.as_str(), json);
        assert_eq!(serde_json::to_string(&metadata).unwrap(), json);
    }

    #[test]
    fn accepts_an_object_at_the_size_limit() {
        let json = object_of_size(MAX_TX_METADATA_SIZE);
        assert_eq!(json.len(), MAX_TX_METADATA_SIZE);

        let metadata = parse(&json).unwrap();

        assert_eq!(metadata.as_str(), json);
    }

    #[test]
    fn rejects_an_object_over_the_size_limit() {
        let message = parse(&object_of_size(MAX_TX_METADATA_SIZE + 1))
            .unwrap_err()
            .to_string();

        assert!(message.contains("more than the limit"), "{message}");
    }

    #[test_case("null" ; "null")]
    #[test_case("[1, 2]" ; "array")]
    #[test_case(r#""metadata""# ; "string")]
    #[test_case("1" ; "number")]
    fn rejects_non_objects(json: &str) {
        let message = parse(json).unwrap_err().to_string();

        assert!(message.contains("JSON object"), "{message}");
    }

    #[test_case(r#"{"k": }"# ; "missing value")]
    #[test_case(r#"{"k": 1"# ; "unterminated")]
    #[test_case("{\"k\": \"a\u{0}b\"}" ; "raw nul")]
    fn rejects_invalid_json(json: &str) {
        parse(json).unwrap_err();
    }
}
