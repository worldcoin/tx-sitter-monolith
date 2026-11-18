use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use alloy::primitives::{Address, B256};
use base_api_types::DecimalU256;
use config::FileFormat;
use eyre::Context;
use serde::{Deserialize, Serialize};

use crate::keys::local_keys::signing_key_from_hex;
use crate::keys::KmsKeyId;
use crate::types::secret_string::SecretString;
use crate::types::secret_url::SecretUrl;

mod env_substitution;

/// Loads the config from `config_files`, later files overriding earlier ones,
/// and from `TX_SITTER__*` environment variables, which override every file.
///
/// Every `${NAME}` in a string value of a config file is replaced by the
/// environment variable `NAME`, so that secrets such as RPC URLs can stay out
/// of the files. A reference to an unset variable fails loading.
///
/// The cross-field invariants are checked separately, see
/// [`Config::validate`].
pub fn load_config<'a>(
    config_files: impl Iterator<Item = &'a Path>,
) -> eyre::Result<Config> {
    let mut settings = config::Config::builder();

    for config_file in config_files {
        let contents = read_config_file(config_file).with_context(|| {
            format!("Reading config file {}", config_file.display())
        })?;

        settings = settings
            .add_source(config::File::from_str(&contents, FileFormat::Toml));
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

    let config = serde_path_to_error::deserialize(settings)?;

    Ok(config)
}

/// Returns the TOML in `path` with its environment variable references
/// substituted.
fn read_config_file(path: &Path) -> eyre::Result<String> {
    let mut value: toml::Value = std::fs::read_to_string(path)?.parse()?;

    env_substitution::substitute_env_vars(&mut value, &|name| {
        std::env::var(name)
    })?;

    Ok(toml::to_string(&value)?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Config {
    pub service: TxSitterConfig,
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub keys: KeysConfig,

    /// The networks the service sends transactions on
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub networks: Vec<NetworkConfig>,

    /// Every relayer the service knows. A relayer in the database but not
    /// in this list is disabled at startup, and loses its API keys.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relayers: Vec<RelayerConfig>,
}

impl Config {
    /// Checks the invariants that deserialization can't express, and reports
    /// every violation in one error.
    ///
    /// Events must not outlive the transactions they describe. The networks
    /// must have distinct chain ids, at least one HTTP RPC, and HTTP and WS
    /// URLs of the right schemes. The relayers must have distinct ids in the
    /// form the API uses, distinct addresses on each chain, belong to a
    /// configured network, queue more transactions than they keep in flight,
    /// and have keys of the kind `keys` selects.
    pub fn validate(&self) -> eyre::Result<()> {
        let mut errors = vec![];

        let TxSitterConfig {
            tx_retention,
            event_retention,
            config_revision,
            ..
        } = &self.service;
        if event_retention > tx_retention {
            errors.push(format!(
                "service.event_retention ({}) must not exceed service.tx_retention ({})",
                humantime::format_duration(*event_retention),
                humantime::format_duration(*tx_retention),
            ));
        }

        if config_revision
            .is_some_and(|revision| i64::try_from(revision).is_err())
        {
            errors.push(format!(
                "service.config_revision must be at most {}",
                i64::MAX
            ));
        }

        let mut chain_ids = HashSet::new();
        for (index, network) in self.networks.iter().enumerate() {
            let mut error = |message: &str| {
                errors.push(format!(
                    "networks[{index}] (chain id {}): {message}",
                    network.chain_id
                ));
            };

            if !chain_ids.insert(network.chain_id) {
                error("duplicate chain id");
            }
            if network.http_rpcs.is_empty() {
                error("http_rpcs is empty");
            }
            for (rpc_index, rpc) in network.http_rpcs.iter().enumerate() {
                if !matches!(rpc.expose().scheme(), "http" | "https") {
                    error(&format!(
                        "http_rpcs[{rpc_index}] is not an http or https URL"
                    ));
                }
            }
            if !matches!(network.ws_rpc.expose().scheme(), "ws" | "wss") {
                error("ws_rpc is not a ws or wss URL");
            }
        }

        let mut relayer_ids = HashSet::new();
        let mut accounts = HashMap::new();
        for (index, relayer) in self.relayers.iter().enumerate() {
            let mut error = |message: &str| {
                errors.push(format!(
                    "relayers[{index}] ({}): {message}",
                    relayer.id
                ));
            };

            let is_canonical_uuid = uuid::Uuid::parse_str(&relayer.id)
                .is_ok_and(|id| id.to_hyphenated().to_string() == relayer.id);
            if !is_canonical_uuid {
                error("id is not a lowercase hyphenated UUID");
            }
            if !relayer_ids.insert(relayer.id.as_str()) {
                error("duplicate relayer id");
            }
            if let Some(first) =
                accounts.insert((relayer.chain_id, relayer.address), index)
            {
                error(&format!(
                    "relayers[{first}] has the same chain id and address, so they would allocate the same nonces"
                ));
            }
            if !chain_ids.contains(&relayer.chain_id) {
                error(&format!(
                    "chain id {} is not in networks",
                    relayer.chain_id
                ));
            }
            if relayer.max_queued_txs <= relayer.max_inflight_txs {
                error(&format!(
                    "max_queued_txs ({}) must be greater than max_inflight_txs ({})",
                    relayer.max_queued_txs, relayer.max_inflight_txs
                ));
            }
            match (&relayer.key, &self.keys) {
                (RelayerKey::Kms(_), KeysConfig::Kms(_)) => {}
                (RelayerKey::Local(key), KeysConfig::Local(_)) => {
                    if signing_key_from_hex(key.expose()).is_err() {
                        error("key.local is not a hex-encoded secp256k1 private key");
                    }
                }
                (RelayerKey::Kms(_), KeysConfig::Local(_)) => {
                    error("key.kms requires keys.kind = \"kms\"");
                }
                (RelayerKey::Local(_), KeysConfig::Kms(_)) => {
                    error("key.local requires keys.kind = \"local\"");
                }
            }
        }

        if !errors.is_empty() {
            eyre::bail!("Invalid config:\n- {}", errors.join("\n- "));
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct TxSitterConfig {
    /// Which parts of the service this process runs
    #[serde(default)]
    pub role: ServiceRole,

    /// The revision of `networks` and `relayers`, which a deploy raises with
    /// every rollout. A process refuses to start with a lower revision than
    /// the one the database applied last, so a process with a stale config
    /// can't undo a newer one, see [`crate::static_config`]. Without a
    /// revision the config is applied unconditionally, for local
    /// development.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_revision: Option<u64>,

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct NetworkConfig {
    pub chain_id: u64,
    pub name: String,

    /// HTTP RPC URLs in order of preference
    pub http_rpcs: Vec<SecretUrl>,

    /// The RPC URL of the block subscription
    pub ws_rpc: SecretUrl,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct RelayerConfig {
    /// A lowercase hyphenated UUID. API keys embed it, so it must never
    /// change.
    pub id: String,
    pub name: String,
    pub chain_id: u64,
    pub key: RelayerKey,

    /// The address `key` must derive, checked at startup
    #[serde(serialize_with = "serialize_checksummed")]
    pub address: Address,

    #[serde(default = "default::max_inflight_txs")]
    pub max_inflight_txs: u64,

    /// Must be greater than `max_inflight_txs`
    #[serde(default = "default::max_queued_txs")]
    pub max_queued_txs: u64,

    /// The relayer stops broadcasting while the gas price of its chain is
    /// above any of these values
    #[serde(default)]
    pub gas_price_limits: Vec<GasPriceLimitConfig>,

    /// A disabled relayer rejects new transactions and broadcasts nothing
    #[serde(default = "default::enabled")]
    pub enabled: bool,

    /// SHA3-256 hashes of the secrets of the relayer's API keys, the only API
    /// keys it accepts
    #[serde(default)]
    pub api_key_hashes: Vec<B256>,
}

fn serialize_checksummed<S: serde::Serializer>(
    address: &Address,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_str(&address.to_checksum(None))
}

/// The key a relayer signs with
///
/// Its `Debug` output hides the key id and the private key, so that neither
/// reaches the logs.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum RelayerKey {
    /// A KMS key id or key ARN, for `keys.kind = "kms"`. A key of another
    /// AWS account needs its key ARN. Aliases are refused, see [`KmsKeyId`].
    Kms(KmsKeyId),
    /// A hex-encoded secp256k1 private key, for `keys.kind = "local"`
    Local(SecretString),
}

impl RelayerKey {
    /// Returns the id the keys source loads the key by.
    pub fn key_id(&self) -> &str {
        match self {
            Self::Kms(key_id) => key_id.as_str(),
            Self::Local(private_key) => private_key.expose(),
        }
    }
}

impl std::fmt::Debug for RelayerKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self {
            Self::Kms(_) => "Kms",
            Self::Local(_) => "Local",
        };

        f.debug_tuple(kind).field(&"********").finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct GasPriceLimitConfig {
    /// Not used: a limit applies to the gas price of the relayer's own chain
    pub chain_id: u64,
    /// In wei, as a decimal string
    pub value: DecimalU256,
}

// Unknown fields are ignored rather than denied, so that deployments can
// keep setting `server.username` and `server.password`: an image from before
// the static config serves `/1/admin/*` to anyone while they are unset, so
// they must stay set while rolling back to such an image is possible.
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

    // Optional address to show in API explorer
    pub server_address: Option<String>,
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
    /// SDK retries attempts that take longer than 2s.
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

    pub fn max_inflight_txs() -> u64 {
        5
    }

    pub fn max_queued_txs() -> u64 {
        20
    }

    pub fn enabled() -> bool {
        true
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
    use test_case::test_case;

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
                config_revision: None,
                escalation_interval: Duration::from_secs(60 * 60),
                max_escalations: default::max_escalations(),
                soft_reorg_interval: default::soft_reorg_interval(),
                hard_reorg_interval: default::hard_reorg_interval(),
                block_stream_timeout: default::block_stream_timeout(),
                rpc_timeout: default::rpc_timeout(),
                task_stall_timeout: default::task_stall_timeout(),
                tx_retention: default::tx_retention(),
                event_retention: default::event_retention(),
                telemetry: None,
            },
            server: ServerConfig {
                host: SocketAddr::from(([127, 0, 0, 1], 3000)),
                shutdown_timeout: default::shutdown_timeout(),
                max_event_waiters: default::max_event_waiters(),
                server_address: None,
            },
            database: DatabaseConfig::connection_string(
                "postgres://postgres:postgres@127.0.0.1:52804/database"
                    .to_string(),
            ),
            keys: KeysConfig::Local(LocalKeysConfig::default()),
            networks: vec![],
            relayers: vec![],
        };

        let toml = toml::to_string_pretty(&config).unwrap();

        assert_eq!(toml, WITH_DB_CONNECTION_STRING);
    }

    #[test]
    fn with_db_parts() {
        let config = Config {
            service: TxSitterConfig {
                role: ServiceRole::All,
                config_revision: None,
                escalation_interval: Duration::from_secs(60 * 60),
                max_escalations: default::max_escalations(),
                soft_reorg_interval: default::soft_reorg_interval(),
                hard_reorg_interval: default::hard_reorg_interval(),
                block_stream_timeout: default::block_stream_timeout(),
                rpc_timeout: default::rpc_timeout(),
                task_stall_timeout: default::task_stall_timeout(),
                tx_retention: default::tx_retention(),
                event_retention: default::event_retention(),
                telemetry: None,
            },
            server: ServerConfig {
                host: SocketAddr::from(([127, 0, 0, 1], 3000)),
                shutdown_timeout: default::shutdown_timeout(),
                max_event_waiters: default::max_event_waiters(),
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
            networks: vec![],
            relayers: vec![],
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
            "Invalid config:\n- service.event_retention (30days 1s) must not exceed service.tx_retention (30days)"
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

    const RELAYER_ID: &str = "6b0b6d1c-7b0e-4c2a-9d3e-2f1a5c4b3a29";
    const OTHER_RELAYER_ID: &str = "1b908a34-5dc1-4d2d-a146-5eb46e975830";

    /// One of anvil's default dev accounts
    const LOCAL_KEY: &str =
        "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    const STATIC_CONFIG: &str = indoc! {r#"
        [service]
        escalation_interval = "1m"

        [server]
        host = "127.0.0.1:3000"

        [database]
        kind = "connection_string"
        connection_string = "postgres://postgres:postgres@127.0.0.1:5432/database"

        [keys]
        kind = "kms"

        [[networks]]
        chain_id = 480
        name = "worldchain"
        http_rpcs = ["${TX_SITTER_TEST_RPC_URL}", "https://fallback.example.com/${TX_SITTER_TEST_RPC_KEY}"]
        ws_rpc = "wss://ws.example.com/${TX_SITTER_TEST_RPC_KEY}"

        [[relayers]]
        id = "6b0b6d1c-7b0e-4c2a-9d3e-2f1a5c4b3a29"
        name = "wc_enableSafe4337Module_1"
        chain_id = 480
        key = { kms = "arn:aws:kms:eu-central-1:123456789012:key/0b3e2a5c" }
        address = "0x1Ed53d680B8890DAe2a63f673a85fFDE1FD5C7a2"
        max_inflight_txs = 10
        max_queued_txs = 50
        gas_price_limits = [{ chain_id = 480, value = "100000000000" }]
        enabled = false
        api_key_hashes = ["0x46239dbfe5502b9f82c3dff992927d8d9b3168e732b4fd5771288569f5a1813d"]

        [[relayers]]
        id = "1b908a34-5dc1-4d2d-a146-5eb46e975830"
        name = "defaults"
        chain_id = 480
        key = { kms = "0b3e2a5c-0000-4000-8000-000000000000" }
        address = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
    "#};

    /// Writes `toml` to a new file and returns its path.
    fn config_file(toml: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir()
            .join(format!("tx-sitter-config-{}.toml", uuid::Uuid::new_v4()));
        std::fs::write(&path, toml).unwrap();

        path
    }

    fn url(url: &str) -> SecretUrl {
        SecretUrl::new(url.parse().unwrap())
    }

    #[test]
    fn loads_networks_and_relayers_with_env_vars() {
        std::env::set_var(
            "TX_SITTER_TEST_RPC_URL",
            "https://rpc.example.com/v2/secret",
        );
        std::env::set_var("TX_SITTER_TEST_RPC_KEY", "fallback-secret");
        let path = config_file(STATIC_CONFIG);

        let config = load_config(std::iter::once(path.as_path())).unwrap();
        std::fs::remove_file(path).unwrap();

        assert_eq!(
            config.networks,
            vec![NetworkConfig {
                chain_id: 480,
                name: "worldchain".to_string(),
                http_rpcs: vec![
                    url("https://rpc.example.com/v2/secret"),
                    url("https://fallback.example.com/fallback-secret"),
                ],
                ws_rpc: url("wss://ws.example.com/fallback-secret"),
            }]
        );
        assert_eq!(
            config.relayers,
            vec![
                RelayerConfig {
                    id: RELAYER_ID.to_string(),
                    name: "wc_enableSafe4337Module_1".to_string(),
                    chain_id: 480,
                    key: RelayerKey::Kms(
                        "arn:aws:kms:eu-central-1:123456789012:key/0b3e2a5c"
                            .parse()
                            .unwrap()
                    ),
                    address: alloy::primitives::address!(
                        "1Ed53d680B8890DAe2a63f673a85fFDE1FD5C7a2"
                    ),
                    max_inflight_txs: 10,
                    max_queued_txs: 50,
                    gas_price_limits: vec![GasPriceLimitConfig {
                        chain_id: 480,
                        value: DecimalU256(alloy::primitives::U256::from(
                            100_000_000_000u64
                        )),
                    }],
                    enabled: false,
                    api_key_hashes: vec![alloy::primitives::b256!(
                        "46239dbfe5502b9f82c3dff992927d8d9b3168e732b4fd5771288569f5a1813d"
                    )],
                },
                RelayerConfig {
                    id: "1b908a34-5dc1-4d2d-a146-5eb46e975830".to_string(),
                    name: "defaults".to_string(),
                    chain_id: 480,
                    key: RelayerKey::Kms(
                        "0b3e2a5c-0000-4000-8000-000000000000".parse().unwrap()
                    ),
                    address: alloy::primitives::address!(
                        "f39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
                    ),
                    max_inflight_txs: default::max_inflight_txs(),
                    max_queued_txs: default::max_queued_txs(),
                    gas_price_limits: vec![],
                    enabled: true,
                    api_key_hashes: vec![],
                },
            ]
        );
        config.validate().unwrap();
    }

    #[test]
    fn loading_fails_on_an_unset_env_var() {
        let path = config_file(indoc! {r#"
            [[networks]]
            http_rpcs = ["${TX_SITTER_TEST_UNSET_RPC_URL}"]
        "#});

        let err = load_config(std::iter::once(path.as_path())).unwrap_err();
        std::fs::remove_file(path).unwrap();

        assert!(
            format!("{err:#}").contains(
                "networks[0].http_rpcs[0]: environment variable TX_SITTER_TEST_UNSET_RPC_URL is not set"
            ),
            "{err:#}"
        );
    }

    #[test]
    fn loading_errors_hide_malformed_rpc_urls() {
        let path = config_file(indoc! {r#"
            [[networks]]
            chain_id = 1
            name = "chain"
            http_rpcs = ["rpc.example.com/v2/provider-key"]
            ws_rpc = "wss://rpc.example.com/v2/provider-key"
        "#});

        let err = load_config(std::iter::once(path.as_path())).unwrap_err();
        std::fs::remove_file(path).unwrap();

        assert_eq!(
            format!("{err:#}"),
            "networks[0].http_rpcs[0]: invalid URL: relative URL without a base"
        );
    }

    #[test]
    fn debug_output_hides_relayer_keys() {
        let kms = RelayerKey::Kms(
            "arn:aws:kms:eu-central-1:123456789012:key/0b3e2a5c"
                .parse()
                .unwrap(),
        );
        let local = RelayerKey::Local(SecretString::new(LOCAL_KEY.to_string()));

        assert_eq!(format!("{kms:?}"), r#"Kms("********")"#);
        assert_eq!(format!("{local:?}"), r#"Local("********")"#);
    }

    /// Parses the config of one relayer whose fields are valid, except for
    /// `field`, which replaces the valid field of the same name.
    fn parse_relayer(field: &str) -> Result<(), String> {
        let (field_name, _) = field.split_once(" = ").unwrap();
        let valid_fields = [
            format!("id = \"{RELAYER_ID}\""),
            "name = \"relayer\"".to_string(),
            "chain_id = 1".to_string(),
            "key = { kms = \"key-id\" }".to_string(),
            "address = \"0x1Ed53d680B8890DAe2a63f673a85fFDE1FD5C7a2\""
                .to_string(),
        ];
        let fields: Vec<String> = valid_fields
            .into_iter()
            .filter(|valid| !valid.starts_with(&format!("{field_name} ")))
            .chain(std::iter::once(field.to_string()))
            .collect();

        let settings = config::Config::builder()
            .add_source(config::File::from_str(
                &format!("[[relayers]]\n{}", fields.join("\n")),
                FileFormat::Toml,
            ))
            .build()
            .unwrap();

        #[derive(Debug, Deserialize)]
        struct Relayers {
            #[allow(dead_code)]
            relayers: Vec<RelayerConfig>,
        }
        serde_path_to_error::deserialize::<_, Relayers>(settings)
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    #[test]
    fn parses_a_relayer() {
        parse_relayer("name = \"relayer\"").unwrap();
    }

    #[test_case(
        "address = \"0x1234\"",
        "relayers[0].address: invalid string length" ;
        "short address"
    )]
    #[test_case(
        "address = \"0xzzd53d680B8890DAe2a63f673a85fFDE1FD5C7a2\"",
        "relayers[0].address: invalid character 'z' at position 0" ;
        "non hex address"
    )]
    #[test_case(
        "key = \"key-id\"",
        "relayers[0].key: enum RelayerKey does not have variant constructor key-id" ;
        "key without a kind"
    )]
    #[test_case(
        "key = { hsm = \"a\" }",
        "relayers[0].key: enum RelayerKey does not have variant constructor hsm" ;
        "unknown key kind"
    )]
    #[test_case(
        "key = { kms = \"arn:aws:kms:us-east-1:key/0b3e2a5c\" }",
        "relayers[0].key.kms: not a KMS key id or key ARN: an ARN has six parts separated by ':'" ;
        "kms key ARN without an account"
    )]
    #[test_case(
        "key = { kms = \"alias/relayer\" }",
        "relayers[0].key.kms: not a KMS key id or key ARN: aliases are refused, because they can be pointed at another key" ;
        "kms alias"
    )]
    #[test_case(
        "key = { kms = \"a\", local = \"b\" }",
        "relayers[0].key: value of enum RelayerKey should be represented by either string or table with exactly one key" ;
        "two key kinds"
    )]
    #[test_case(
        "api_key_hashes = [\"0x1234\"]",
        "relayers[0].api_key_hashes[0]: invalid string length" ;
        "short api key hash"
    )]
    #[test_case(
        "gas_price_limits = [{ chain_id = 1, value = \"1.5\" }]",
        "relayers[0].gas_price_limits[0].value: invalid value: string \"1.5\", expected a decimal number" ;
        "fractional gas price limit"
    )]
    #[test_case(
        "max_inflight_tx = 5",
        "relayers[0].max_inflight_tx: unknown field `max_inflight_tx`, expected one of `id`, `name`, `chain_id`, `key`, `address`, `max_inflight_txs`, `max_queued_txs`, `gas_price_limits`, `enabled`, `api_key_hashes`" ;
        "misspelled field"
    )]
    fn rejects_a_malformed_relayer(field: &str, expected_error: &str) {
        assert_eq!(parse_relayer(field).unwrap_err(), expected_error);
    }

    fn network(chain_id: u64) -> NetworkConfig {
        NetworkConfig {
            chain_id,
            name: format!("chain {chain_id}"),
            http_rpcs: vec![url("https://rpc.example.com")],
            ws_rpc: url("wss://rpc.example.com"),
        }
    }

    fn relayer(id: &str, chain_id: u64) -> RelayerConfig {
        RelayerConfig {
            id: id.to_string(),
            name: "relayer".to_string(),
            chain_id,
            key: RelayerKey::Local(SecretString::new(LOCAL_KEY.to_string())),
            address: Address::ZERO,
            max_inflight_txs: 5,
            max_queued_txs: 20,
            gas_price_limits: vec![],
            enabled: true,
            api_key_hashes: vec![],
        }
    }

    fn static_config(
        keys: KeysConfig,
        networks: Vec<NetworkConfig>,
        relayers: Vec<RelayerConfig>,
    ) -> Config {
        let mut config: Config =
            toml::from_str(WITH_DB_CONNECTION_STRING).unwrap();
        config.keys = keys;
        config.networks = networks;
        config.relayers = relayers;

        config
    }

    fn local_keys() -> KeysConfig {
        KeysConfig::Local(LocalKeysConfig::default())
    }

    #[test]
    fn accepts_a_config_without_networks_and_relayers() {
        static_config(local_keys(), vec![], vec![])
            .validate()
            .unwrap();
    }

    #[test]
    fn accepts_valid_networks_and_relayers() {
        static_config(
            local_keys(),
            vec![network(1), network(480)],
            vec![relayer(RELAYER_ID, 1), relayer(OTHER_RELAYER_ID, 480)],
        )
        .validate()
        .unwrap();
    }

    #[test]
    fn rejects_a_revision_that_doesnt_fit_the_database() {
        let mut config = static_config(local_keys(), vec![], vec![]);
        config.service.config_revision = Some(u64::MAX);

        let err = config.validate().unwrap_err();

        assert_eq!(
            err.to_string(),
            "Invalid config:\n- service.config_revision must be at most 9223372036854775807"
        );
    }

    #[test_case(
        vec![network(1), network(1)],
        vec![],
        "networks[1] (chain id 1): duplicate chain id" ;
        "duplicate chain id"
    )]
    #[test_case(
        vec![NetworkConfig { http_rpcs: vec![], ..network(1) }],
        vec![],
        "networks[0] (chain id 1): http_rpcs is empty" ;
        "no http rpc"
    )]
    #[test_case(
        vec![NetworkConfig { http_rpcs: vec![url("https://a.example.com"), url("wss://b.example.com")], ..network(1) }],
        vec![],
        "networks[0] (chain id 1): http_rpcs[1] is not an http or https URL" ;
        "ws url as http rpc"
    )]
    #[test_case(
        vec![NetworkConfig { ws_rpc: url("https://ws.example.com"), ..network(1) }],
        vec![],
        "networks[0] (chain id 1): ws_rpc is not a ws or wss URL" ;
        "http url as ws rpc"
    )]
    #[test_case(
        vec![network(1)],
        vec![relayer(RELAYER_ID, 1), RelayerConfig { address: Address::with_last_byte(1), ..relayer(RELAYER_ID, 1) }],
        "relayers[1] (6b0b6d1c-7b0e-4c2a-9d3e-2f1a5c4b3a29): duplicate relayer id" ;
        "duplicate relayer id"
    )]
    #[test_case(
        vec![network(1)],
        vec![relayer(RELAYER_ID, 1), RelayerConfig { enabled: false, ..relayer(OTHER_RELAYER_ID, 1) }],
        "relayers[1] (1b908a34-5dc1-4d2d-a146-5eb46e975830): relayers[0] has the same chain id and address, so they would allocate the same nonces" ;
        "duplicate address on a chain"
    )]
    #[test_case(
        vec![network(1)],
        vec![relayer(RELAYER_ID, 480)],
        "relayers[0] (6b0b6d1c-7b0e-4c2a-9d3e-2f1a5c4b3a29): chain id 480 is not in networks" ;
        "unknown chain"
    )]
    #[test_case(
        vec![network(1)],
        vec![relayer("relayer-1", 1)],
        "relayers[0] (relayer-1): id is not a lowercase hyphenated UUID" ;
        "id that is not a uuid"
    )]
    #[test_case(
        vec![network(1)],
        vec![relayer("6B0B6D1C-7B0E-4C2A-9D3E-2F1A5C4B3A29", 1)],
        "relayers[0] (6B0B6D1C-7B0E-4C2A-9D3E-2F1A5C4B3A29): id is not a lowercase hyphenated UUID" ;
        "uppercase uuid"
    )]
    #[test_case(
        vec![network(1)],
        vec![RelayerConfig { max_inflight_txs: 20, max_queued_txs: 20, ..relayer(RELAYER_ID, 1) }],
        "relayers[0] (6b0b6d1c-7b0e-4c2a-9d3e-2f1a5c4b3a29): max_queued_txs (20) must be greater than max_inflight_txs (20)" ;
        "queue no larger than inflight"
    )]
    #[test_case(
        vec![network(1)],
        vec![RelayerConfig { key: RelayerKey::Local(SecretString::new("abcd".to_string())), ..relayer(RELAYER_ID, 1) }],
        "relayers[0] (6b0b6d1c-7b0e-4c2a-9d3e-2f1a5c4b3a29): key.local is not a hex-encoded secp256k1 private key" ;
        "invalid local key"
    )]
    #[test_case(
        vec![network(1)],
        vec![RelayerConfig { key: RelayerKey::Kms("key-id".parse().unwrap()), ..relayer(RELAYER_ID, 1) }],
        "relayers[0] (6b0b6d1c-7b0e-4c2a-9d3e-2f1a5c4b3a29): key.kms requires keys.kind = \"kms\"" ;
        "kms key with local keys"
    )]
    fn rejects(
        networks: Vec<NetworkConfig>,
        relayers: Vec<RelayerConfig>,
        expected_error: &str,
    ) {
        let err = static_config(local_keys(), networks, relayers)
            .validate()
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            format!("Invalid config:\n- {expected_error}")
        );
    }

    #[test]
    fn rejects_a_local_key_with_kms_keys() {
        let kms_keys = KeysConfig::Kms(KmsKeysConfig {
            kms_timeout: default::kms_timeout(),
        });

        let err = static_config(
            kms_keys,
            vec![network(1)],
            vec![relayer(RELAYER_ID, 1)],
        )
        .validate()
        .unwrap_err();

        assert_eq!(
            err.to_string(),
            format!("Invalid config:\n- relayers[0] ({RELAYER_ID}): key.local requires keys.kind = \"local\"")
        );
    }

    #[test]
    fn reports_every_error() {
        let err = static_config(
            local_keys(),
            vec![NetworkConfig {
                http_rpcs: vec![],
                ..network(1)
            }],
            vec![relayer("relayer-1", 480)],
        )
        .validate()
        .unwrap_err();

        assert_eq!(
            err.to_string(),
            indoc! {"
                Invalid config:
                - networks[0] (chain id 1): http_rpcs is empty
                - relayers[0] (relayer-1): id is not a lowercase hyphenated UUID
                - relayers[0] (relayer-1): chain id 480 is not in networks"
            }
        );
    }
}
