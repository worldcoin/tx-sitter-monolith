use std::borrow::Cow;
use std::str::FromStr;

use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use serde::Serialize;
use sha3::{Digest, Sha3_256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKey {
    pub relayer_id: String,
    pub api_key: [u8; 32],
}

impl ApiKey {
    pub fn new(relayer_id: impl ToString, key: [u8; 32]) -> Self {
        let relayer_id = relayer_id.to_string();

        Self {
            relayer_id,
            api_key: key,
        }
    }

    pub fn random(relayer_id: impl ToString) -> Self {
        let relayer_id = relayer_id.to_string();

        let mut api_key = [0u8; 32];
        OsRng.fill_bytes(&mut api_key);

        Self {
            relayer_id,
            api_key,
        }
    }

    pub fn api_key_hash(&self) -> [u8; 32] {
        Sha3_256::digest(self.api_key).into()
    }
}

impl Serialize for ApiKey {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
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

        if buffer.len() != 48 {
            return Err(eyre::eyre!("invalid api key"));
        }

        let relayer_id = uuid::Uuid::from_slice(&buffer[..16])?;
        let relayer_id = relayer_id.to_string();

        let mut api_key = [0u8; 32];
        api_key.copy_from_slice(&buffer[16..]);

        Ok(Self {
            relayer_id,
            api_key,
        })
    }
}

impl std::fmt::Display for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut buffer = [0u8; 48];

        let relayer_id = uuid::Uuid::parse_str(&self.relayer_id)
            .map_err(|_| std::fmt::Error)?;

        buffer[..16].copy_from_slice(relayer_id.as_bytes());
        buffer[16..].copy_from_slice(&self.api_key);

        let encoded = base64::prelude::BASE64_URL_SAFE.encode(buffer);

        write!(f, "{}", encoded)
    }
}

#[cfg(test)]
mod tests {
    use rand::rngs::OsRng;
    use rand::RngCore;

    use super::*;

    fn random_api_key() -> ApiKey {
        let mut api_key = [0u8; 32];
        OsRng.fill_bytes(&mut api_key);

        ApiKey::new(uuid::Uuid::new_v4().to_string(), api_key)
    }

    #[test]
    fn from_to_str() {
        let api_key = random_api_key();

        let api_key_str = api_key.to_string();

        println!("api_key_str = {api_key_str}");

        let api_key_parsed = api_key_str.parse::<ApiKey>().unwrap();

        assert_eq!(api_key, api_key_parsed);
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
