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
            if username != self.0.username || password != self.0.password {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        Config, DatabaseConfig, KeysConfig, LocalKeysConfig, ServerConfig,
        TxSitterConfig,
    };
    use crate::types::secret_string::SecretString;
    use std::net::SocketAddr;
    use std::time::Duration;

    fn test_config(username: Option<&str>, password: Option<&str>) -> Config {
        Config {
            service: TxSitterConfig {
                escalation_interval: Duration::from_secs(60),
                max_escalations: 10,
                soft_reorg_interval: Duration::from_secs(60),
                hard_reorg_interval: Duration::from_secs(3600),
                block_stream_timeout: Duration::from_secs(60),
                predefined: None,
                telemetry: None,
            },
            server: ServerConfig {
                host: SocketAddr::from(([127, 0, 0, 1], 3000)),
                username: username.map(|s| SecretString::new(s.to_string())),
                password: password.map(|s| SecretString::new(s.to_string())),
                server_address: None,
            },
            database: DatabaseConfig::connection_string("postgres://test"),
            keys: KeysConfig::Local(LocalKeysConfig::default()),
        }
    }

    fn create_basic_auth(username: &str, password: &str) -> BasicAuth {
        BasicAuth(auth::Basic {
            username: username.to_string(),
            password: password.to_string(),
        })
    }

    /// Wrapper for testing that only exposes config (which is all validate() needs)
    struct TestApp {
        config: Config,
    }

    impl AsRef<App> for TestApp {
        fn as_ref(&self) -> &App {
            // SAFETY: validate() only accesses app.config.server.credentials()
            // Config is the first field in App, so pointer cast is valid
            unsafe { &*std::ptr::from_ref(&self.config).cast::<App>() }
        }
    }

    #[tokio::test]
    async fn validate_succeeds_when_no_credentials_configured() {
        let test_app = TestApp {
            config: test_config(None, None),
        };
        let auth = create_basic_auth("any", "any");

        assert!(auth.validate(&test_app).await.is_ok());
    }

    #[tokio::test]
    async fn validate_succeeds_with_correct_credentials() {
        let test_app = TestApp {
            config: test_config(Some("admin"), Some("secret")),
        };
        let auth = create_basic_auth("admin", "secret");

        assert!(auth.validate(&test_app).await.is_ok());
    }

    #[tokio::test]
    async fn validate_fails_with_wrong_username() {
        let test_app = TestApp {
            config: test_config(Some("admin"), Some("secret")),
        };
        let auth = create_basic_auth("wrong", "secret");

        assert!(auth.validate(&test_app).await.is_err());
    }

    #[tokio::test]
    async fn validate_fails_with_wrong_password() {
        let test_app = TestApp {
            config: test_config(Some("admin"), Some("secret")),
        };
        let auth = create_basic_auth("admin", "wrong");

        assert!(auth.validate(&test_app).await.is_err());
    }

    #[tokio::test]
    async fn validate_fails_with_both_wrong() {
        let test_app = TestApp {
            config: test_config(Some("admin"), Some("secret")),
        };
        let auth = create_basic_auth("wrong", "wrong");

        assert!(auth.validate(&test_app).await.is_err());
    }
}
