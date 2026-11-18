use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A KMS key, as a key id or a key ARN.
///
/// KMS resolves key ids in the caller's AWS account, so only a key ARN
/// identifies a key of another account, such as a relayer key created by
/// tx-sitter-aws. Both forms identify a key in the region of the KMS client.
///
/// Aliases, as alias names or alias ARNs, are refused: an alias can be
/// pointed at another key, and the service would then sign with a key whose
/// address it never checked.
///
/// Parsing checks the form only, not that the key exists or that the service
/// may use it:
///
/// ```
/// use tx_sitter::keys::KmsKeyId;
///
/// let key: KmsKeyId =
///     "arn:aws:kms:us-east-1:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab"
///         .parse()
///         .unwrap();
/// assert!("alias/relayer".parse::<KmsKeyId>().is_err());
/// assert!("arn:aws:kms:us-east-1:key/1234abcd".parse::<KmsKeyId>().is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct KmsKeyId(String);

impl KmsKeyId {
    /// Returns the key in the form it was parsed from, which is what KMS
    /// requests take.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why a string is not a [`KmsKeyId`]
///
/// It never contains the string, which identifies a key and so stays out of
/// logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidKmsKeyId(&'static str);

impl std::fmt::Display for InvalidKmsKeyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not a KMS key id or key ARN: {}", self.0)
    }
}

impl std::error::Error for InvalidKmsKeyId {}

impl FromStr for KmsKeyId {
    type Err = InvalidKmsKeyId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.starts_with("arn:") {
            check_arn(value)?;
        } else if value.starts_with("alias/") {
            return Err(ALIAS_REFUSED);
        } else {
            check_key_id(value)?;
        }

        Ok(Self(value.to_string()))
    }
}

impl TryFrom<String> for KmsKeyId {
    type Error = InvalidKmsKeyId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<KmsKeyId> for String {
    fn from(key: KmsKeyId) -> Self {
        key.0
    }
}

const ALIAS_REFUSED: InvalidKmsKeyId = InvalidKmsKeyId(
    "aliases are refused, because they can be pointed at another key",
);

/// Checks `arn:<partition>:kms:<region>:<account id>:key/<key id>`.
fn check_arn(arn: &str) -> Result<(), InvalidKmsKeyId> {
    let parts: Vec<&str> = arn.splitn(6, ':').collect();
    let [_, partition, service, region, account_id, resource] = parts[..]
    else {
        return Err(InvalidKmsKeyId("an ARN has six parts separated by ':'"));
    };

    if !partition.starts_with("aws") || !is_lowercase_name(partition) {
        return Err(InvalidKmsKeyId(
            "the ARN's partition is not an AWS partition",
        ));
    }
    if service != "kms" {
        return Err(InvalidKmsKeyId("the ARN is not of a KMS resource"));
    }
    if !is_lowercase_name(region) {
        return Err(InvalidKmsKeyId("the ARN has no valid region"));
    }
    if account_id.len() != 12
        || !account_id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(InvalidKmsKeyId("the ARN's account id is not 12 digits"));
    }

    if let Some(key_id) = resource.strip_prefix("key/") {
        check_key_id(key_id)
    } else if resource.starts_with("alias/") {
        Err(ALIAS_REFUSED)
    } else {
        Err(InvalidKmsKeyId("the ARN's resource is not key/"))
    }
}

fn check_key_id(key_id: &str) -> Result<(), InvalidKmsKeyId> {
    let is_valid = !key_id.is_empty()
        && key_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');

    if !is_valid {
        return Err(InvalidKmsKeyId(
            "a key id has only letters, digits and '-'",
        ));
    }

    Ok(())
}

fn is_lowercase_name(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'
        })
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case("1234abcd-12ab-34cd-56ef-1234567890ab" ; "key id")]
    #[test_case("mrk-1234abcd12ab34cd56ef1234567890ab" ; "multi-region key id")]
    #[test_case(
        "arn:aws:kms:us-east-1:592080917095:key/1234abcd-12ab-34cd-56ef-1234567890ab" ;
        "key ARN of another account"
    )]
    #[test_case(
        "arn:aws-us-gov:kms:us-gov-west-1:111122223333:key/mrk-1234abcd12ab34cd56ef1234567890ab" ;
        "multi-region key ARN in another partition"
    )]
    fn parses(key: &str) {
        let parsed: KmsKeyId = key.parse().unwrap();

        assert_eq!(parsed.as_str(), key);
    }

    #[test_case("", "a key id has only letters, digits and '-'" ; "empty")]
    #[test_case(
        " 1234abcd-12ab-34cd-56ef-1234567890ab",
        "a key id has only letters, digits and '-'" ;
        "padded key id"
    )]
    #[test_case(
        "alias/tx-sitter/relayer_1",
        "aliases are refused, because they can be pointed at another key" ;
        "alias name"
    )]
    #[test_case(
        "arn:aws:kms:us-east-1:111122223333:alias/relayer",
        "aliases are refused, because they can be pointed at another key" ;
        "alias ARN"
    )]
    #[test_case(
        "arn:aws:kms:us-east-1:key/1234abcd",
        "an ARN has six parts separated by ':'" ;
        "ARN without an account"
    )]
    #[test_case(
        "arn:aws:iam:us-east-1:111122223333:key/1234abcd",
        "the ARN is not of a KMS resource" ;
        "ARN of another service"
    )]
    #[test_case(
        "arn:gcp:kms:us-east-1:111122223333:key/1234abcd",
        "the ARN's partition is not an AWS partition" ;
        "ARN of another partition"
    )]
    #[test_case(
        "arn:aws:kms::111122223333:key/1234abcd",
        "the ARN has no valid region" ;
        "ARN without a region"
    )]
    #[test_case(
        "arn:aws:kms:us-east-1:1111-2222-3333:key/1234abcd",
        "the ARN's account id is not 12 digits" ;
        "ARN with a formatted account id"
    )]
    #[test_case(
        "arn:aws:kms:us-east-1:111122223333:1234abcd",
        "the ARN's resource is not key/" ;
        "ARN without a resource type"
    )]
    #[test_case(
        "arn:aws:kms:us-east-1:111122223333:key/",
        "a key id has only letters, digits and '-'" ;
        "ARN without a key id"
    )]
    #[test_case(
        "arn:aws:kms:us-east-1:111122223333:key/1234abcd:extra",
        "a key id has only letters, digits and '-'" ;
        "ARN with trailing parts"
    )]
    fn rejects(key: &str, reason: &str) {
        let err = key.parse::<KmsKeyId>().unwrap_err();

        assert_eq!(
            err.to_string(),
            format!("not a KMS key id or key ARN: {reason}")
        );
    }

    #[test]
    fn serializes_as_the_parsed_string() {
        let key = "arn:aws:kms:us-east-1:111122223333:key/1234abcd";
        let parsed: KmsKeyId = serde_json::from_value(key.into()).unwrap();

        assert_eq!(serde_json::to_value(&parsed).unwrap(), key);
    }
}
