use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use config::FileFormat;
use serde::{Deserialize, Serialize};

use crate::api_key::ApiKey;

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

    #[serde(with = "humantime_serde", default = "default_soft_reorg_interval")]
    pub soft_reorg_interval: Duration,

    #[serde(with = "humantime_serde", default = "default_hard_reorg_interval")]
    pub hard_reorg_interval: Duration,

    #[serde(default)]
    pub datadog_enabled: bool,

    #[serde(default)]
    pub statsd_enabled: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predefined: Option<Predefined>,
}

const fn default_soft_reorg_interval() -> Duration {
    Duration::from_secs(60)
}

const fn default_hard_reorg_interval() -> Duration {
    Duration::from_secs(60 * 60)
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

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LocalKeysConfig {}

impl KeysConfig {
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
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
        datadog_enabled = false
        statsd_enabled = false

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
        datadog_enabled = false
        statsd_enabled = false

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
                soft_reorg_interval: default_soft_reorg_interval(),
                hard_reorg_interval: default_hard_reorg_interval(),
                datadog_enabled: false,
                statsd_enabled: false,
                predefined: None,
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
                soft_reorg_interval: default_soft_reorg_interval(),
                hard_reorg_interval: default_hard_reorg_interval(),
                datadog_enabled: false,
                statsd_enabled: false,
                predefined: None,
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
        std::env::set_var("TX_SITTER__SERVICE__DATADOG_ENABLED", "true");
        std::env::set_var("TX_SITTER__SERVICE__STATSD_ENABLED", "true");
        std::env::set_var("TX_SITTER__SERVER__HOST", "0.0.0.0:8080");
        std::env::set_var("TX_SITTER__SERVER__USERNAME", "authUsername");
        std::env::set_var("TX_SITTER__SERVER__PASSWORD", "authPassword");
        std::env::set_var("TX_SITTER__KEYS__KIND", "kms");

        let config = load_config(std::iter::empty()).unwrap();

        assert_eq!(config.service.statsd_enabled, true);
        assert_eq!(config.service.datadog_enabled, true);
        assert_eq!(config.service.escalation_interval, Duration::from_secs(60));
        assert_eq!(
            config.database.to_connection_string(),
            "postgres://dbUsername:dbPassword@dbHost:dbPort/dbName"
        );
    }
}
