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

impl Config {
    /// Checks the settings that depend on each other.
    pub fn validate(&self) -> eyre::Result<()> {
        let TxSitterConfig {
            tx_retention,
            event_retention,
            ..
        } = &self.service;
        eyre::ensure!(
            event_retention <= tx_retention,
            "service.event_retention ({}) must not exceed service.tx_retention ({})",
            humantime::format_duration(*event_retention),
            humantime::format_duration(*tx_retention),
        );

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TxSitterConfig {
    /// Which parts of the service this process runs
    #[serde(default)]
    pub role: ServiceRole,

    #[serde(with = "humantime_serde")]
    pub escalation_interval: Duration,

    /// Maximum number of escalations of a given transaction
    #[serde(default = "default::max_escalations")]
    pub max_escalations: usize,

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

    /// Max amount of time to wait for a response to an HTTP RPC request
    #[serde(with = "humantime_serde", default = "default::rpc_timeout")]
    pub rpc_timeout: Duration,

    /// Max amount of time a background task may go without reporting
    /// progress before it is considered stalled and fails the liveness probe
    #[serde(with = "humantime_serde", default = "default::task_stall_timeout")]
    pub task_stall_timeout: Duration,

    /// How long finalized transactions are kept after they are mined.
    ///
    /// Sending a `txId` again through the v2 API is idempotent only while its
    /// transaction is kept. Once it is pruned, the id is free again and a
    /// repeat creates a new transaction.
    #[serde(with = "humantime_serde", default = "default::tx_retention")]
    pub tx_retention: Duration,

    /// How long events stay in the feed of `GET /2/events`.
    ///
    /// A consumer whose cursor is older gets 410, because it may have missed
    /// events.
    ///
    /// At most `tx_retention`, so that an event's transaction and its
    /// metadata are kept about as long as the event, and its id can't be
    /// reused by another transaction while the event exists. Its `finalized`
    /// event, stored about an hour after the transaction is mined, can outlive
    /// it by that hour.
    #[serde(with = "humantime_serde", default = "default::event_retention")]
    pub event_retention: Duration,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predefined: Option<Predefined>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<TelemetryConfig>,
}

/// The parts of the service a process runs.
///
/// Background tasks must run in exactly one process per database, so a
/// deployment runs any number of [`ServiceRole::Api`] processes next to a
/// single [`ServiceRole::Worker`] or [`ServiceRole::All`] process.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ServiceRole {
    /// Serves the HTTP API and runs the background tasks
    #[default]
    All,
    /// Serves the HTTP API and runs none of the background tasks that
    /// process transactions
    Api,
    /// Runs the background tasks and serves only the health endpoints
    Worker,
}

impl ServiceRole {
    pub fn serves_api(self) -> bool {
        matches!(self, Self::All | Self::Api)
    }

    pub fn runs_tasks(self) -> bool {
        matches!(self, Self::All | Self::Worker)
    }
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

    /// Max amount of time to wait for in-flight requests and background
    /// tasks to finish after shutdown is requested
    #[serde(with = "humantime_serde", default = "default::shutdown_timeout")]
    pub shutdown_timeout: Duration,

    /// Max number of `GET /2/events` requests that wait for events at once.
    /// Further ones get 503 until one finishes.
    #[serde(default = "default::max_event_waiters")]
    pub max_event_waiters: usize,

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
pub struct KmsKeysConfig {
    /// Max amount of time a KMS operation may take, retries included. The
    /// SDK retries attempts that take longer than 2s, except for CreateKey,
    /// which gets a single attempt.
    #[serde(with = "humantime_serde", default = "default::kms_timeout")]
    pub kms_timeout: Duration,
}

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

    pub fn rpc_timeout() -> Duration {
        Duration::from_secs(10)
    }

    pub fn kms_timeout() -> Duration {
        Duration::from_secs(5)
    }

    pub fn task_stall_timeout() -> Duration {
        Duration::from_secs(5 * 60)
    }

    pub fn shutdown_timeout() -> Duration {
        Duration::from_secs(20)
    }

    pub fn tx_retention() -> Duration {
        Duration::from_secs(30 * 24 * 60 * 60)
    }

    pub fn event_retention() -> Duration {
        Duration::from_secs(7 * 24 * 60 * 60)
    }

    pub fn max_event_waiters() -> usize {
        64
    }

    pub fn max_escalations() -> usize {
        100
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
        role = "all"
        escalation_interval = "1h"
        max_escalations = 100
        soft_reorg_interval = "1m"
        hard_reorg_interval = "1h"
        block_stream_timeout = "1m"
        rpc_timeout = "10s"
        task_stall_timeout = "5m"
        tx_retention = "30days"
        event_retention = "7days"

        [server]
        host = "127.0.0.1:3000"
        shutdown_timeout = "20s"
        max_event_waiters = 64

        [database]
        kind = "connection_string"
        connection_string = "postgres://postgres:postgres@127.0.0.1:52804/database"

        [keys]
        kind = "local"
    "#};

    const WITH_DB_PARTS: &str = indoc! {r#"
        [service]
        role = "all"
        escalation_interval = "1h"
        max_escalations = 100
        soft_reorg_interval = "1m"
        hard_reorg_interval = "1h"
        block_stream_timeout = "1m"
        rpc_timeout = "10s"
        task_stall_timeout = "5m"
        tx_retention = "30days"
        event_retention = "7days"

        [server]
        host = "127.0.0.1:3000"
        shutdown_timeout = "20s"
        max_event_waiters = 64

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
                role: ServiceRole::All,
                escalation_interval: Duration::from_secs(60 * 60),
                max_escalations: default::max_escalations(),
                soft_reorg_interval: default::soft_reorg_interval(),
                hard_reorg_interval: default::hard_reorg_interval(),
                block_stream_timeout: default::block_stream_timeout(),
                rpc_timeout: default::rpc_timeout(),
                task_stall_timeout: default::task_stall_timeout(),
                tx_retention: default::tx_retention(),
                event_retention: default::event_retention(),
                predefined: None,
                telemetry: None,
            },
            server: ServerConfig {
                host: SocketAddr::from(([127, 0, 0, 1], 3000)),
                shutdown_timeout: default::shutdown_timeout(),
                max_event_waiters: default::max_event_waiters(),
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
                role: ServiceRole::All,
                escalation_interval: Duration::from_secs(60 * 60),
                max_escalations: default::max_escalations(),
                soft_reorg_interval: default::soft_reorg_interval(),
                hard_reorg_interval: default::hard_reorg_interval(),
                block_stream_timeout: default::block_stream_timeout(),
                rpc_timeout: default::rpc_timeout(),
                task_stall_timeout: default::task_stall_timeout(),
                tx_retention: default::tx_retention(),
                event_retention: default::event_retention(),
                predefined: None,
                telemetry: None,
            },
            server: ServerConfig {
                host: SocketAddr::from(([127, 0, 0, 1], 3000)),
                shutdown_timeout: default::shutdown_timeout(),
                max_event_waiters: default::max_event_waiters(),
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
    fn events_must_not_outlive_their_transactions() {
        let mut config: Config =
            toml::from_str(WITH_DB_CONNECTION_STRING).unwrap();
        config.validate().unwrap();

        config.service.event_retention = config.service.tx_retention;
        config.validate().unwrap();

        config.service.event_retention += Duration::from_secs(1);
        let err = config.validate().unwrap_err();
        assert_eq!(
            err.to_string(),
            "service.event_retention (30days 1s) must not exceed service.tx_retention (30days)"
        );
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
        std::env::set_var("TX_SITTER__SERVICE__ROLE", "worker");
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
        assert_eq!(config.service.rpc_timeout, default::rpc_timeout());
        assert_eq!(config.service.tx_retention, default::tx_retention());
        assert_eq!(config.service.event_retention, default::event_retention());
        assert_eq!(
            config.server.max_event_waiters,
            default::max_event_waiters()
        );
        let KeysConfig::Kms(kms_config) = &config.keys else {
            panic!("Expected KMS keys config");
        };
        assert_eq!(kms_config.kms_timeout, default::kms_timeout());
        assert_eq!(config.service.role, ServiceRole::Worker);
        assert_eq!(
            config.database.to_connection_string(),
            "postgres://dbUsername:dbPassword@dbHost:dbPort/dbName"
        );
    }
}
