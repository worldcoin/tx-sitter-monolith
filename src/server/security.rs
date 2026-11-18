use poem::http::StatusCode;
use poem::Result;
use poem_openapi::{auth, SecurityScheme};

use crate::api_key::ApiKey;
use crate::app::App;
use crate::db::data::ApiKeyStatus;

/// An API key sent in an `Authorization: Bearer <api key>` header
#[derive(SecurityScheme)]
#[oai(ty = "bearer")]
pub struct BearerApiKey(auth::Bearer);

impl BearerApiKey {
    /// Returns the API key once the database confirms it and its relayer is
    /// enabled.
    ///
    /// Like an API key in a v1 URL, a key that doesn't parse or isn't stored
    /// fails with 401, and a key of a disabled relayer with 403.
    pub async fn authorize(&self, app: impl AsRef<App>) -> Result<ApiKey> {
        let api_key = self.parse()?;

        api_key.validate(app).await?;

        Ok(api_key)
    }

    /// Returns the API key once the database confirms it, whether or not its
    /// relayer is enabled, for an operation that checks that itself.
    ///
    /// A key that doesn't parse or isn't stored fails with 401.
    pub async fn authenticate(&self, app: impl AsRef<App>) -> Result<ApiKey> {
        let api_key = self.parse()?;

        match api_key.status(app).await? {
            ApiKeyStatus::Valid | ApiKeyStatus::RelayerDisabled => Ok(api_key),
            ApiKeyStatus::Unknown => Err(unauthorized()),
        }
    }

    fn parse(&self) -> Result<ApiKey> {
        self.0.token.parse().map_err(|_| unauthorized())
    }
}

impl ApiKey {
    /// Checks that the key's relayer accepts the key and is enabled.
    ///
    /// Fails with 401 for a key the relayer doesn't accept, and with 403
    /// while the relayer is disabled.
    pub async fn validate(&self, app: impl AsRef<App>) -> Result<()> {
        match self.status(app).await? {
            ApiKeyStatus::Valid => Ok(()),
            ApiKeyStatus::Unknown => Err(unauthorized()),
            ApiKeyStatus::RelayerDisabled => {
                Err(poem::error::Error::from_string(
                    "Relayer is disabled".to_string(),
                    StatusCode::FORBIDDEN,
                ))
            }
        }
    }

    async fn status(&self, app: impl AsRef<App>) -> Result<ApiKeyStatus> {
        app.as_ref().api_key_status(self).await.map_err(|err| {
            poem::error::Error::from_string(
                err.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })
    }
}

fn unauthorized() -> poem::Error {
    poem::error::Error::from_string(
        "Unauthorized".to_string(),
        StatusCode::UNAUTHORIZED,
    )
}
