use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use url::Url;

/// A URL that may embed a secret, such as an RPC URL that carries a provider
/// API key.
///
/// Neither its `Debug` output nor its deserialization errors contain the
/// URL, so it can't leak into logs.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SecretUrl(Url);

impl SecretUrl {
    #[must_use]
    pub fn new(url: Url) -> Self {
        Self(url)
    }

    #[must_use]
    pub fn expose(&self) -> &Url {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SecretUrl {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let url = String::deserialize(deserializer)?;

        url.parse()
            .map(Self)
            .map_err(|err| D::Error::custom(format!("invalid URL: {err}")))
    }
}

impl fmt::Debug for SecretUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("********")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_the_url() {
        let url = SecretUrl::new(
            "https://eth.example.com/v2/provider-key".parse().unwrap(),
        );

        assert_eq!(format!("{url:?}"), "********");
    }

    #[test]
    fn deserialization_errors_hide_the_url() {
        let err = serde_json::from_str::<SecretUrl>(
            r#""eth.example.com/v2/provider-key""#,
        )
        .unwrap_err();

        assert_eq!(err.to_string(), "invalid URL: relative URL without a base");
    }
}
