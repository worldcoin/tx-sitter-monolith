use poem::http::StatusCode;
use poem::Result;
use poem_openapi::{auth, SecurityScheme};

use crate::api_key::ApiKey;
use crate::app::App;

#[derive(SecurityScheme)]
#[oai(ty = "basic")]
pub struct BasicAuth(auth::Basic);

impl BasicAuth {
    pub async fn validate(&self, app: impl AsRef<App>) -> Result<()> {
        let credentials = app.as_ref().config.server.credentials();
        Self::check_credentials(credentials, &self.0.username, &self.0.password)
    }

    fn check_credentials(
        expected: Option<(&str, &str)>,
        username: &str,
        password: &str,
    ) -> Result<()> {
        if let Some((expected_user, expected_pass)) = expected {
            if expected_user != username || expected_pass != password {
                return Err(unauthorized());
            }
        }

        Ok(())
    }
}

/// An API key sent in an `Authorization: Bearer <api key>` header
#[derive(SecurityScheme)]
#[oai(ty = "bearer")]
pub struct BearerApiKey(auth::Bearer);

impl BearerApiKey {
    /// Returns the API key once the database confirms it.
    ///
    /// Like an API key in a v1 URL, a key that doesn't parse or isn't stored
    /// fails with 401.
    pub async fn authorize(&self, app: impl AsRef<App>) -> Result<ApiKey> {
        let api_key: ApiKey =
            self.0.token.parse().map_err(|_| unauthorized())?;

        api_key.validate(app).await?;

        Ok(api_key)
    }
}

impl ApiKey {
    pub async fn validate(&self, app: impl AsRef<App>) -> Result<()> {
        let app = app.as_ref();

        let is_authorized = app.is_authorized(self).await.map_err(|err| {
            poem::error::Error::from_string(
                err.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;

        if !is_authorized {
            return Err(unauthorized());
        }

        Ok(())
    }
}

fn unauthorized() -> poem::Error {
    poem::error::Error::from_string(
        "Unauthorized".to_string(),
        StatusCode::UNAUTHORIZED,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_credentials_succeeds_when_none_configured() {
        assert!(BasicAuth::check_credentials(None, "any", "any").is_ok());
    }

    #[test]
    fn check_credentials_succeeds_with_correct_credentials() {
        let expected = Some(("admin", "secret"));
        assert!(
            BasicAuth::check_credentials(expected, "admin", "secret").is_ok()
        );
    }

    #[test]
    fn check_credentials_fails_with_wrong_username() {
        let expected = Some(("admin", "secret"));
        assert!(
            BasicAuth::check_credentials(expected, "wrong", "secret").is_err()
        );
    }

    #[test]
    fn check_credentials_fails_with_wrong_password() {
        let expected = Some(("admin", "secret"));
        assert!(
            BasicAuth::check_credentials(expected, "admin", "wrong").is_err()
        );
    }

    #[test]
    fn check_credentials_fails_with_both_wrong() {
        let expected = Some(("admin", "secret"));
        assert!(
            BasicAuth::check_credentials(expected, "wrong", "wrong").is_err()
        );
    }
}
