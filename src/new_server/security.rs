use poem::Result;
use poem_openapi::{auth, SecurityScheme};

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
