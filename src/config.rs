use std::net::SocketAddr;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Config {
    pub service: TxSitterConfig,
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub keys: KeysConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TxSitterConfig {
    #[serde(with = "humantime_serde")]
    pub escalation_interval: Duration,

    #[serde(default)]
    pub datadog_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ServerConfig {
    pub host: SocketAddr,

    pub username: Option<String>,
    pub password: Option<String>,
}

impl ServerConfig {
    pub fn credentials(&self) -> Option<(&str, &str)> {
        let username = self.username.as_deref()?;
        let password = self.password.as_deref()?;

        Some((username, password))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DatabaseConfig {
    ConnectionString(DbConnectionString),
    Parts(DbParts),
}

impl DatabaseConfig {
    pub fn connection_string(s: impl ToString) -> Self {
        Self::ConnectionString(DbConnectionString {
            connection_string: s.to_string(),
        })
    }

    pub fn to_connection_string(&self) -> String {
        match self {
            Self::ConnectionString(s) => s.connection_string.clone(),
            Self::Parts(parts) => {
                format!(
                    "postgres://{}:{}@{}:{}/{}",
                    parts.username,
                    parts.password,
                    parts.host,
                    parts.port,
                    parts.database
                )
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DbConnectionString {
    pub connection_string: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DbParts {
    pub host: String,
    pub port: String,
    pub username: String,
    pub password: String,
    pub database: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum KeysConfig {
    Kms(KmsKeysConfig),
    Local(LocalKeysConfig),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct KmsKeysConfig {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LocalKeysConfig {}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::*;

    const WITH_DB_CONNECTION_STRING: &str = indoc! {r#"
        [service]
        escalation_interval = "1h"
        datadog_enabled = false

        [server]
        host = "127.0.0.1:3000"

        [database]
        kind = "connection_string"
        connection_string = "postgres://postgres:postgres@127.0.0.1:52804/database"

        [keys]
        kind = "local"
    "#};

    const WITH_DB_PARTS: &str = indoc! {r#"
        [service]
        escalation_interval = "1h"
        datadog_enabled = false

        [server]
        host = "127.0.0.1:3000"

        [database]
        kind = "parts"
        host = "host"
        port = "5432"
        username = "user"
        password = "pass"
        database = "db"

        [keys]
        kind = "local"
    "#};

    #[test]
    fn with_db_connection_string() {
        let config = Config {
            service: TxSitterConfig {
                escalation_interval: Duration::from_secs(60 * 60),
                datadog_enabled: false,
            },
            server: ServerConfig {
                host: SocketAddr::from(([127, 0, 0, 1], 3000)),
                username: None,
                password: None,
            },
            database: DatabaseConfig::connection_string(
                "postgres://postgres:postgres@127.0.0.1:52804/database"
                    .to_string(),
            ),
            keys: KeysConfig::Local(LocalKeysConfig {}),
        };

        let toml = toml::to_string_pretty(&config).unwrap();

        assert_eq!(toml, WITH_DB_CONNECTION_STRING);
    }

    #[test]
    fn with_db_parts() {
        let config = Config {
            service: TxSitterConfig {
                escalation_interval: Duration::from_secs(60 * 60),
                datadog_enabled: false,
            },
            server: ServerConfig {
                host: SocketAddr::from(([127, 0, 0, 1], 3000)),
                username: None,
                password: None,
            },
            database: DatabaseConfig::Parts(DbParts {
                host: "host".to_string(),
                port: "5432".to_string(),
                username: "user".to_string(),
                password: "pass".to_string(),
                database: "db".to_string(),
            }),
            keys: KeysConfig::Local(LocalKeysConfig {}),
        };

        let toml = toml::to_string_pretty(&config).unwrap();

        assert_eq!(toml, WITH_DB_PARTS);
    }
}
