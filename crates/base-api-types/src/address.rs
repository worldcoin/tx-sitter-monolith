use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{ParseError, ParseFromJSON, ToJSON};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Address(pub ethers::types::Address);

impl From<ethers::types::Address> for Address {
    fn from(value: ethers::types::Address) -> Self {
        Self(value)
    }
}

impl poem_openapi::types::Type for Address {
    const IS_REQUIRED: bool = true;

    type RawValueType = ethers::types::Address;

    type RawElementValueType = ethers::types::Address;

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

#[cfg(test)]
mod tests {
    use ethers::types::H160;
    use hex_literal::hex;

    use super::*;

    #[test]
    fn deserialize() {
        let address: Address = serde_json::from_str(
            r#""1Ed53d680B8890DAe2a63f673a85fFDE1FD5C7a2""#,
        )
        .unwrap();

        let expected = H160(hex!("1Ed53d680B8890DAe2a63f673a85fFDE1FD5C7a2"));

        assert_eq!(address.0, expected);
    }
}
