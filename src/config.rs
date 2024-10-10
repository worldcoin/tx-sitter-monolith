use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use config::FileFormat;
use serde::{Deserialize, Serialize};

use crate::api_key::ApiKey;
use crate::types::secret_string::SecretString;

pub fn load_config<'a>(
    config_files: impl Iterator<Item = &'a Path>,
) -> eyre::Result<Config> {
    let mut settings = config::Config::builder();

    for config_file in config_files {
        settings = settings.add_source(
            config::File::from(config_file).format(FileFormat::Toml),
        );
    }

    let settings = settings
        .add_source(
            config::Environment::with_prefix("TX_SITTER").separator("__"),
        )
        .add_source(
            config::Environment::with_prefix("TX_SITTER_EXT")
                .separator("__")
                .try_parsing(true)
                .list_separator(","),
        )
        .build()?;

    let config = settings.try_deserialize::<Config>()?;

    Ok(config)
}

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

    #[serde(
        with = "humantime_serde",
        default = "default::soft_reorg_interval"
    )]
    pub soft_reorg_interval: Duration,

    #[serde(
        with = "humantime_serde",
        default = "default::hard_reorg_interval"
    )]
    pub hard_reorg_interval: Duration,

    /// Max amount of time to wait for a new block from the RPC block stream
    #[serde(
        with = "humantime_serde",
        default = "default::block_stream_timeout"
    )]
    pub block_stream_timeout: Duration,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predefined: Option<Predefined>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<TelemetryConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Predefined {
    pub network: PredefinedNetwork,
    pub relayer: PredefinedRelayer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PredefinedNetwork {
    pub chain_id: u64,
    pub name: String,
    pub http_rpc: String,
    pub ws_rpc: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PredefinedRelayer {
    pub id: String,
    pub name: String,
    pub key_id: String,
    pub chain_id: u64,
    pub api_key: ApiKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ServerConfig {
    pub host: SocketAddr,

    pub username: Option<SecretString>,
    pub password: Option<SecretString>,

    // Optional address to show in API explorer
    pub server_address: Option<String>,
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
            connection_string: SecretString::new(s.to_string()),
        })
    }

    pub fn to_connection_string(&self) -> String {
        match self {
            Self::ConnectionString(s) => s.connection_string.clone().into(),
            Self::Parts(parts) => {
                format!(
                    "postgres://{}:{}@{}:{}/{}",
                    parts.username.expose(),
                    parts.password.expose(),
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
    pub connection_string: SecretString,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DbParts {
    pub host: String,
    pub port: String,
    pub username: SecretString,
    pub password: SecretString,
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

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LocalKeysConfig {}

impl KeysConfig {
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    // Service name - used for logging, metrics and tracing
    pub service_name: String,
    // Traces
    pub traces_endpoint: Option<String>,
    // Metrics
    pub metrics: Option<MetricsConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfig {
    #[serde(default = "default::metrics::host")]
    pub host: String,
    #[serde(default = "default::metrics::port")]
    pub port: u16,
    #[serde(default = "default::metrics::queue_size")]
    pub queue_size: usize,
    #[serde(default = "default::metrics::buffer_size")]
    pub buffer_size: usize,
    pub prefix: String,
}

mod default {
    use super::*;

    pub fn soft_reorg_interval() -> Duration {
        Duration::from_secs(60)
    }

    pub fn hard_reorg_interval() -> Duration {
        Duration::from_secs(60 * 60)
    }

    pub fn block_stream_timeout() -> Duration {
        Duration::from_secs(60)
    }

    pub mod metrics {
        pub fn host() -> String {
            "127.0.0.1".to_string()
        }

        pub fn port() -> u16 {
            8125
        }

        pub fn queue_size() -> usize {
            5000
        }

        pub fn buffer_size() -> usize {
            256
        }
    }
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::*;

    const WITH_DB_CONNECTION_STRING: &str = indoc! {r#"
        [service]
        escalation_interval = "1h"
        soft_reorg_interval = "1m"
        hard_reorg_interval = "1h"
        block_stream_timeout = "1m"

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
        soft_reorg_interval = "1m"
        hard_reorg_interval = "1h"
        block_stream_timeout = "1m"

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
                soft_reorg_interval: default::soft_reorg_interval(),
                hard_reorg_interval: default::hard_reorg_interval(),
                block_stream_timeout: default::block_stream_timeout(),
                predefined: None,
                telemetry: None,
            },
            server: ServerConfig {
                host: SocketAddr::from(([127, 0, 0, 1], 3000)),
                username: None,
                password: None,
                server_address: None,
            },
            database: DatabaseConfig::connection_string(
                "postgres://postgres:postgres@127.0.0.1:52804/database"
                    .to_string(),
            ),
            keys: KeysConfig::Local(LocalKeysConfig::default()),
        };

        let toml = toml::to_string_pretty(&config).unwrap();

        assert_eq!(toml, WITH_DB_CONNECTION_STRING);
    }

    #[test]
    fn with_db_parts() {
        let config = Config {
            service: TxSitterConfig {
                escalation_interval: Duration::from_secs(60 * 60),
                soft_reorg_interval: default::soft_reorg_interval(),
                hard_reorg_interval: default::hard_reorg_interval(),
                block_stream_timeout: default::block_stream_timeout(),
                predefined: None,
                telemetry: None,
            },
            server: ServerConfig {
                host: SocketAddr::from(([127, 0, 0, 1], 3000)),
                username: None,
                password: None,
                server_address: None,
            },
            database: DatabaseConfig::Parts(DbParts {
                host: "host".to_string(),
                port: "5432".to_string(),
                username: SecretString::new("user".to_string()),
                password: SecretString::new("pass".to_string()),
                database: "db".to_string(),
            }),
            keys: KeysConfig::Local(LocalKeysConfig::default()),
        };

        let toml = toml::to_string_pretty(&config).unwrap();

        assert_eq!(toml, WITH_DB_PARTS);
    }

    #[test]
    fn env_config_test() {
        std::env::set_var("TX_SITTER__DATABASE__KIND", "parts");
        std::env::set_var("TX_SITTER__DATABASE__HOST", "dbHost");
        std::env::set_var("TX_SITTER__DATABASE__PORT", "dbPort");
        std::env::set_var("TX_SITTER__DATABASE__DATABASE", "dbName");
        std::env::set_var("TX_SITTER__DATABASE__USERNAME", "dbUsername");
        std::env::set_var("TX_SITTER__DATABASE__PASSWORD", "dbPassword");
        std::env::set_var("TX_SITTER__SERVICE__ESCALATION_INTERVAL", "1m");
        std::env::set_var(
            "TX_SITTER__SERVICE__TELEMETRY__SERVICE_NAME",
            "tx-sitter",
        );
        std::env::set_var("TX_SITTER__SERVER__HOST", "0.0.0.0:8080");
        std::env::set_var("TX_SITTER__SERVER__USERNAME", "authUsername");
        std::env::set_var("TX_SITTER__SERVER__PASSWORD", "authPassword");
        std::env::set_var("TX_SITTER__KEYS__KIND", "kms");

        let config = load_config(std::iter::empty()).unwrap();

        assert_eq!(
            config.service.telemetry.as_ref().unwrap().service_name,
            "tx-sitter"
        );
        assert_eq!(config.service.escalation_interval, Duration::from_secs(60));
        assert_eq!(
            config.database.to_connection_string(),
            "postgres://dbUsername:dbPassword@dbHost:dbPort/dbName"
        );
    }
}
