use std::fmt;

use serde::{Deserialize, Serialize};
use url::Url;

/// A URL that may embed a secret, such as an RPC URL that carries a provider
/// API key.
///
/// Its `Debug` output hides the URL, so it can't leak into logs.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}
