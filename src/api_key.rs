use std::borrow::Cow;
use std::str::FromStr;

use base64::Engine;
use poem_openapi::registry::{MetaSchema, MetaSchemaRef};
use poem_openapi::types::{
    ParseError, ParseFromJSON, ParseFromParameter, ToJSON,
};
use rand::rngs::OsRng;
use rand::Rng;
use serde::Serialize;
use sha3::{Digest, Sha3_256};

const DEFAULT_SECRET_LEN: usize = 16;
const MIN_SECRET_LEN: usize = 16;
const MAX_SECRET_LEN: usize = 32;
const UUID_LEN: usize = 16;

#[derive(Clone, Eq, PartialEq)]
struct ApiSecret(Vec<u8>);

/// Derive Serialize manually to avoid leaking the secret.
impl Serialize for ApiSecret {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&"***")
    }
}

/// Derive Debug manually to avoid leaking the secret.
impl std::fmt::Debug for ApiSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ApiSecret").field(&"***").finish()
    }
}

/// Zero out the secret when dropped.
impl Drop for ApiSecret {
    fn drop(&mut self) {
        self.0.iter_mut().for_each(|b| *b = 0);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiKey {
    relayer_id: String,
    secret: ApiSecret,
}

impl ApiKey {
    pub fn new(
        relayer_id: impl ToString,
        secret: Vec<u8>,
    ) -> eyre::Result<Self> {
        if secret.len() < MIN_SECRET_LEN || secret.len() > MAX_SECRET_LEN {
            eyre::bail!("invalid api key");
        }
        let relayer_id = relayer_id.to_string();

        Ok(Self {
            relayer_id,
            secret: ApiSecret(secret),
        })
    }

    pub fn random(relayer_id: impl ToString) -> Self {
        let relayer_id = relayer_id.to_string();

        Self {
            relayer_id,
            secret: ApiSecret(OsRng.gen::<[u8; DEFAULT_SECRET_LEN]>().into()),
        }
    }

    pub fn api_key_secret_hash(&self) -> [u8; 32] {
        Sha3_256::digest(self.secret.0.clone()).into()
    }

    pub fn relayer_id(&self) -> &str {
        &self.relayer_id
    }
}

impl Serialize for ApiKey {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer
            .serialize_str(&self.reveal().map_err(serde::ser::Error::custom)?)
    }
}

impl<'de> serde::Deserialize<'de> for ApiKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        <Cow<'static, str>>::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl FromStr for ApiKey {
    type Err = eyre::ErrReport;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let buffer = base64::prelude::BASE64_URL_SAFE.decode(s)?;

        if buffer.len() < UUID_LEN + MIN_SECRET_LEN
            || buffer.len() > UUID_LEN + MAX_SECRET_LEN
        {
            eyre::bail!("invalid api key");
        }

        let relayer_id = uuid::Uuid::from_slice(&buffer[..UUID_LEN])?;
        let relayer_id = relayer_id.to_string();

        let secret = ApiSecret(buffer[UUID_LEN..].into());

        Ok(Self { relayer_id, secret })
    }
}

impl poem_openapi::types::Type for ApiKey {
    const IS_REQUIRED: bool = true;

    type RawValueType = ApiKey;

    type RawElementValueType = ApiKey;

    fn name() -> std::borrow::Cow<'static, str> {
        "string(api-key)".into()
    }

    fn schema_ref() -> MetaSchemaRef {
        let mut schema_ref = MetaSchema::new_with_format("string", "api-key");

        schema_ref.example = Some(serde_json::Value::String(
            "G5CKNF3BTS2hRl60bpdYMNPqXvXsP-QZd2lrtmgctsk=".to_string(),
        ));
        schema_ref.title = Some("Api Key".to_string());
        schema_ref.description = Some("Base64 encoded API key");

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

impl ParseFromJSON for ApiKey {
    fn parse_from_json(
        value: Option<serde_json::Value>,
    ) -> poem_openapi::types::ParseResult<Self> {
        let value = value.ok_or_else(ParseError::expected_input)?;

        serde_json::from_value(value).map_err(ParseError::custom)
    }
}

impl ToJSON for ApiKey {
    fn to_json(&self) -> Option<serde_json::Value> {
        serde_json::to_value(self).ok()
    }
}

impl ParseFromParameter for ApiKey {
    fn parse_from_parameter(
        value: &str,
    ) -> poem_openapi::types::ParseResult<Self> {
        value.parse().map_err(|_| ParseError::expected_input())
    }
}

impl ApiKey {
    pub fn reveal(&self) -> eyre::Result<String> {
        let relayer_id = uuid::Uuid::parse_str(&self.relayer_id)
            .map_err(|_| std::fmt::Error)?;

        let bytes = relayer_id
            .as_bytes()
            .iter()
            .cloned()
            .chain(self.secret.0.iter().cloned())
            .collect::<Vec<_>>();

        Ok(base64::prelude::BASE64_URL_SAFE.encode(bytes))
    }
}

#[cfg(test)]
mod tests {
    use rand::rngs::OsRng;

    use super::*;

    fn random_api_key() -> ApiKey {
        ApiKey::new(
            uuid::Uuid::new_v4().to_string(),
            OsRng.gen::<[u8; DEFAULT_SECRET_LEN]>().into(),
        )
        .unwrap()
    }

    fn invalid_short_api_key() -> ApiKey {
        let mut buf = [0u8; MAX_SECRET_LEN + 1];
        OsRng.fill(&mut buf[..]);
        ApiKey {
            relayer_id: uuid::Uuid::new_v4().to_string(),
            secret: ApiSecret(buf.into()),
        }
    }

    fn invalid_long_api_key() -> ApiKey {
        let mut buf = [0u8; MAX_SECRET_LEN + 1];
        OsRng.fill(&mut buf[..]);
        ApiKey {
            relayer_id: uuid::Uuid::new_v4().to_string(),
            secret: ApiSecret(buf.into()),
        }
    }

    #[test]
    fn from_to_str() {
        let api_key = random_api_key();

        let api_key_str = api_key.reveal().unwrap();

        println!("api_key_str = {api_key_str}");

        let api_key_parsed = api_key_str.parse::<ApiKey>().unwrap();

        assert_eq!(api_key.relayer_id, api_key_parsed.relayer_id);
        assert_eq!(api_key.secret, api_key_parsed.secret);
    }

    #[test]
    fn assert_api_secret_debug() {
        let api_secret = random_api_key().secret;
        assert_eq!(&format!("{:?}", api_secret), "ApiSecret(\"***\")");
    }

    #[test]
    fn assert_api_key_length_validation() {
        let long_api_key = invalid_long_api_key();

        let _ = ApiKey::new(
            long_api_key.relayer_id.clone(),
            long_api_key.secret.0.clone(),
        )
        .expect_err("long api key should be invalid");

        let _ = ApiKey::from_str(&long_api_key.reveal().unwrap())
            .expect_err("long api key should be invalid");

        let short_api_key = invalid_short_api_key();

        let _ = ApiKey::new(
            short_api_key.relayer_id.clone(),
            short_api_key.secret.0.clone(),
        )
        .expect_err("short api key should be invalid");

        let _ = ApiKey::from_str(&short_api_key.reveal().unwrap())
            .expect_err("short api key should be invalid");
    }

    #[test]
    fn from_to_serde_json() {
        let api_key = random_api_key();

        let api_key_json = serde_json::to_string(&api_key).unwrap();

        println!("api_key_str = {api_key_json}");

        let api_key_parsed: ApiKey =
            serde_json::from_str(&api_key_json).unwrap();

        assert_eq!(api_key, api_key_parsed);
    }

    #[test]
    fn from_to_serde_json_owned() {
        let api_key = random_api_key();

        let api_key_json: serde_json::Value =
            serde_json::to_value(&api_key).unwrap();

        println!("api_key_str = {api_key_json}");

        let api_key_parsed: ApiKey =
            serde_json::from_value(api_key_json).unwrap();

        assert_eq!(api_key, api_key_parsed);
    }
}
