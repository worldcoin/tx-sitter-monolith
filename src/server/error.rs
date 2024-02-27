use axum::response::IntoResponse;
use hyper::StatusCode;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApiError {
    #[error("Invalid key encoding")]
    KeyEncoding,

    #[error("Invalid key length")]
    KeyLength,

    #[error("Unauthorized")]
    Unauthorized,

    #[error("Invalid format")]
    InvalidFormat,

    #[error("Missing tx")]
    MissingTx,

    #[error("Relayer is disabled")]
    RelayerDisabled,

    #[error("Too many queued transactions, max: {max}, current: {current}")]
    TooManyTransactions { max: usize, current: usize },

    #[error("Internal error {0}")]
    #[serde(with = "serde_eyre")]
    Other(#[from] eyre::Report),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status_code = match self {
            Self::KeyLength | Self::KeyEncoding => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Other(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::InvalidFormat => StatusCode::BAD_REQUEST,
            Self::MissingTx => StatusCode::NOT_FOUND,
            Self::RelayerDisabled => StatusCode::FORBIDDEN,
            Self::TooManyTransactions { .. } => StatusCode::TOO_MANY_REQUESTS,
        };

        let message = serde_json::to_string(&self)
            .expect("Failed to serialize error message");

        (status_code, message).into_response()
    }
}

// Mostly used for tests
impl PartialEq for ApiError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::TooManyTransactions {
                    max: l_max,
                    current: l_current,
                },
                Self::TooManyTransactions {
                    max: r_max,
                    current: r_current,
                },
            ) => l_max == r_max && l_current == r_current,
            (Self::Other(l0), Self::Other(r0)) => {
                l0.to_string() == r0.to_string()
            }
            _ => {
                core::mem::discriminant(self) == core::mem::discriminant(other)
            }
        }
    }
}

mod serde_eyre {
    use std::borrow::Cow;

    use serde::Deserialize;

    pub fn serialize<S>(
        error: &eyre::Report,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let error = error.to_string();
        serializer.serialize_str(&error)
    }

    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<eyre::Report, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let error = Cow::<'static, str>::deserialize(deserializer)?;
        Ok(eyre::eyre!(error))
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case(ApiError::KeyLength, r#""keyLength""# ; "Key length")]
    #[test_case(ApiError::Other(eyre::eyre!("Test error")), r#"{"other":"Test error"}"# ; "Other error")]
    #[test_case(ApiError::TooManyTransactions { max: 10, current: 20 }, r#"{"tooManyTransactions":{"max":10,"current":20}}"# ; "Too many transactions")]
    fn serialization(error: ApiError, expected: &str) {
        let serialized = serde_json::to_string(&error).unwrap();

        assert_eq!(serialized, expected);

        let deserialized = serde_json::from_str::<ApiError>(expected).unwrap();

        assert_eq!(error, deserialized);
    }
}
