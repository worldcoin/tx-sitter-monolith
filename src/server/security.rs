use poem::Result;
use poem_openapi::{auth, SecurityScheme};

use crate::api_key::ApiKey;
use crate::app::App;

#[derive(SecurityScheme)]
#[oai(ty = "basic")]
pub struct BasicAuth(auth::Basic);

impl BasicAuth {
    pub async fn validate(&self, app: impl AsRef<App>) -> Result<()> {
        let app = app.as_ref();

        if let Some((username, password)) = app.config.server.credentials() {
            if username != self.0.username && password != self.0.password {
                return Err(poem::error::Error::from_string(
                    "Unauthorized".to_string(),
                    poem::http::StatusCode::UNAUTHORIZED,
                ));
            }
        }

        Ok(())
    }
}

impl ApiKey {
    pub async fn validate(&self, app: impl AsRef<App>) -> Result<()> {
        let app = app.as_ref();

        let is_authorized = app.is_authorized(self).await.map_err(|err| {
            poem::error::Error::from_string(
                err.to_string(),
                poem::http::StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;

        if !is_authorized {
            return Err(poem::error::Error::from_string(
                "Unauthorized".to_string(),
                poem::http::StatusCode::UNAUTHORIZED,
            ));
        }

        Ok(())
    }
}
