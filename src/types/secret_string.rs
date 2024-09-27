use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Serialize};

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretString(String);

impl SecretString {
    #[must_use]
    pub fn new(str: String) -> Self {
        Self(str)
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }

    fn format(&self) -> String {
        "********".to_owned()
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.format().fmt(f)
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.format().fmt(f)
    }
}

impl From<String> for SecretString {
    fn from(str: String) -> Self {
        Self::new(str)
    }
}

impl From<SecretString> for String {
    fn from(secret_string: SecretString) -> Self {
        secret_string.0
    }
}

impl Deref for SecretString {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_expose() {
        let secret = SecretString::from(
            "postgres://user:password@localhost:5432/database".to_string(),
        );
        assert_eq!(
            secret.expose(),
            "postgres://user:password@localhost:5432/database"
        );
    }
}
