use std::cmp::Ordering;
use std::time::Duration;

use alloy::primitives::B256;
use eyre::Context;
use futures::stream::{self, StreamExt};
use sha3::{Digest, Sha3_256};

use crate::app::App;
use crate::config::{NetworkConfig, RelayerConfig};
use crate::db::data::{ConfigChange, Reconciliation};
use crate::keys::kms_keys::is_transient_error;
use crate::keys::{KeysSource, UniversalSigner};
use crate::task_runner::with_jitter;

/// How many relayer keys are loaded at once at startup
const KEY_CHECK_CONCURRENCY: usize = 8;

/// Max amount of time the key check may take, retries included
const KEY_CHECK_TIMEOUT: Duration = Duration::from_secs(60);

/// How many times a key load that fails with a transient KMS error is tried
const KEY_LOAD_MAX_ATTEMPTS: u32 = 5;

/// The backoff before the first retry of a key load, doubled for every
/// further retry
const KEY_LOAD_FIRST_BACKOFF: Duration = Duration::from_millis(500);

/// The revision of a static config and a hash of its networks and relayers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigVersion {
    pub revision: u64,
    pub hash: B256,
}

impl ConfigVersion {
    /// Hashes `networks` and `relayers`, ignoring their order and the order
    /// of each relayer's API keys.
    ///
    /// The hash leaves out RPC URLs, because they come from secrets that may
    /// rotate without a deploy, and every process sends requests to the RPCs
    /// of its own config.
    pub fn new(
        revision: u64,
        networks: &[NetworkConfig],
        relayers: &[RelayerConfig],
    ) -> Self {
        let mut networks: Vec<(u64, &str)> = networks
            .iter()
            .map(|network| (network.chain_id, network.name.as_str()))
            .collect();
        networks.sort();

        let mut relayers = relayers.to_vec();
        relayers.sort_by(|a, b| a.id.cmp(&b.id));
        for relayer in &mut relayers {
            relayer.api_key_hashes.sort();
            relayer.api_key_hashes.dedup();
        }

        let canonical = serde_json::to_vec(&(networks, relayers))
            .expect("The config serializes to JSON");

        Self {
            revision,
            hash: B256::from_slice(&Sha3_256::digest(canonical)),
        }
    }
}

/// What applying a config does to the database
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// Write the config into the database
    Apply,
    /// Leave the database as it is, because it already holds the config
    Skip,
}

/// Decides what applying a config of version `incoming` does to a database
/// that applied a config of version `stored` last.
///
/// A config without a revision, or one for a database that stores no
/// version, is applied. Otherwise a higher revision is applied, the same
/// revision with the same hash is skipped, and a lower revision, or the same
/// revision with another hash, fails: either would undo a config the
/// database applied from a newer or a changed deploy.
pub fn plan(
    stored: Option<ConfigVersion>,
    incoming: Option<ConfigVersion>,
) -> eyre::Result<Plan> {
    let (Some(stored), Some(incoming)) = (stored, incoming) else {
        return Ok(Plan::Apply);
    };

    match incoming.revision.cmp(&stored.revision) {
        Ordering::Greater => Ok(Plan::Apply),
        Ordering::Equal if incoming.hash == stored.hash => Ok(Plan::Skip),
        Ordering::Equal => eyre::bail!(
            "The config has revision {}, which the database applied with \
             other networks or relayers. Raise service.config_revision when \
             the config changes.",
            incoming.revision
        ),
        Ordering::Less => eyre::bail!(
            "The config has revision {}, but the database applied revision \
             {} last, so this config is stale. Deploy again to roll back.",
            incoming.revision,
            stored.revision
        ),
    }
}

/// Writes the networks, relayers and API keys of the config into the
/// database, see [`Database::apply_static_config`], and logs at warn every
/// change, and every disabled relayer that still has unsent or in-flight
/// transactions.
///
/// When the database already holds this config's revision and hash, this
/// does nothing, so that restarts don't depend on KMS. Otherwise it first
/// checks that the key of every enabled relayer derives the configured
/// address, so a config with a wrong key or address fails startup before it
/// changes the database.
///
/// [`Database::apply_static_config`]: crate::db::Database::apply_static_config
pub async fn apply(app: &App) -> eyre::Result<()> {
    let config = &app.config;
    let version = config.service.config_revision.map(|revision| {
        ConfigVersion::new(revision, &config.networks, &config.relayers)
    });

    match version {
        Some(version) => {
            let stored = app.db.get_static_config_version().await?;
            if plan(stored, Some(version))? == Plan::Skip {
                return Ok(());
            }
        }
        None => tracing::warn!(
            "service.config_revision is not set, so the config is applied \
             whatever the database applied before"
        ),
    }

    tokio::time::timeout(KEY_CHECK_TIMEOUT, verify_relayer_keys(app))
        .await
        .map_err(|_| {
            eyre::eyre!(
                "The relayer key check didn't finish within {KEY_CHECK_TIMEOUT:?}"
            )
        })??;

    let reconciliation = app
        .db
        .apply_static_config(&config.networks, &config.relayers, version)
        .await
        .context("Writing the static config to the database")?;
    let Reconciliation::Applied(changes) = reconciliation else {
        return Ok(());
    };

    changes.iter().for_each(log_change);

    let backlogs = app
        .db
        .get_disabled_relayer_backlogs()
        .await
        .context("Reading the transactions of disabled relayers")?;
    for backlog in backlogs {
        tracing::warn!(
            relayer_id = backlog.relayer_id,
            relayer_name = backlog.relayer_name,
            unsent_txs = backlog.unsent_txs,
            inflight_txs = backlog.inflight_txs,
            "A disabled relayer has transactions it won't broadcast or escalate"
        );
    }

    Ok(())
}

fn log_change(change: &ConfigChange) {
    match change {
        ConfigChange::NetworkAdded { chain_id } => {
            tracing::warn!(chain_id, "Added a network");
        }
        ConfigChange::NetworkChanged { chain_id, fields } => {
            tracing::warn!(chain_id, ?fields, "Changed a network");
        }
        ConfigChange::RelayerAdded {
            relayer_id,
            relayer_name,
        } => tracing::warn!(relayer_id, relayer_name, "Added a relayer"),
        ConfigChange::RelayerChanged {
            relayer_id,
            relayer_name,
            fields,
        } => tracing::warn!(
            relayer_id,
            relayer_name,
            ?fields,
            "Changed a relayer"
        ),
        ConfigChange::RelayerEnabled {
            relayer_id,
            relayer_name,
        } => tracing::warn!(relayer_id, relayer_name, "Enabled a relayer"),
        ConfigChange::RelayerDisabled {
            relayer_id,
            relayer_name,
        } => tracing::warn!(relayer_id, relayer_name, "Disabled a relayer"),
        ConfigChange::RelayerRemoved {
            relayer_id,
            relayer_name,
        } => tracing::warn!(
            relayer_id,
            relayer_name,
            "Disabled a relayer that is not in the config"
        ),
        ConfigChange::ApiKeysAdded { relayer_id, count } => {
            tracing::warn!(relayer_id, count, "Added API keys");
        }
        ConfigChange::ApiKeysRevoked { relayer_id, count } => {
            tracing::warn!(relayer_id, count, "Revoked API keys");
        }
    }
}

/// Loads the key of every enabled relayer and fails, listing every relayer at
/// fault, if a key can't be loaded or derives another address than the
/// configured one.
///
/// KMS keys are loaded with `GetPublicKey`, each attempt bounded by
/// `keys.kms_timeout`, see [`load_signer_with_retries`].
async fn verify_relayer_keys(app: &App) -> eyre::Result<()> {
    let errors: Vec<String> = stream::iter(&app.config.relayers)
        .filter(|relayer| std::future::ready(relayer.enabled))
        .map(|relayer| verify_relayer_key(app, relayer))
        .buffer_unordered(KEY_CHECK_CONCURRENCY)
        .filter_map(|result| std::future::ready(result.err()))
        .map(|err| format!("{err:#}"))
        .collect()
        .await;

    if !errors.is_empty() {
        eyre::bail!("Relayer key check failed:\n- {}", errors.join("\n- "));
    }

    Ok(())
}

async fn verify_relayer_key(
    app: &App,
    relayer: &RelayerConfig,
) -> eyre::Result<()> {
    let signer = load_signer_with_retries(app.keys_source.as_ref(), relayer)
        .await
        .with_context(|| format!("relayer {}: loading its key", relayer.id))?;

    let derived_address = signer.address();
    if derived_address != relayer.address {
        eyre::bail!(
            "relayer {}: expected address {}, but its key derives {derived_address}",
            relayer.id,
            relayer.address
        );
    }

    Ok(())
}

/// Loads the key of `relayer`, retrying errors that [`is_transient_error`]
/// accepts up to [`KEY_LOAD_MAX_ATTEMPTS`] times with exponential backoff and
/// jitter.
async fn load_signer_with_retries(
    keys_source: &dyn KeysSource,
    relayer: &RelayerConfig,
) -> eyre::Result<UniversalSigner> {
    let mut backoff = KEY_LOAD_FIRST_BACKOFF;
    let mut attempt = 1;

    loop {
        let err = match keys_source
            .load_signer(relayer.key.key_id().to_string())
            .await
        {
            Ok(signer) => return Ok(signer),
            Err(err) => err,
        };

        if attempt >= KEY_LOAD_MAX_ATTEMPTS || !is_transient_error(&err) {
            return Err(err);
        }

        tracing::warn!(
            relayer_id = relayer.id,
            attempt,
            error = %format!("{err:#}"),
            "Loading a relayer's key failed, retrying"
        );
        tokio::time::sleep(with_jitter(backoff)).await;
        backoff *= 2;
        attempt += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};

    use alloy::primitives::Address;
    use alloy::signers::aws::AwsSignerError;
    use aws_smithy_runtime_api::client::result::SdkError;
    use test_case::test_case;

    use super::*;
    use crate::config::{LocalKeysConfig, RelayerKey};
    use crate::keys::LocalKeys;
    use crate::types::secret_string::SecretString;
    use crate::types::secret_url::SecretUrl;

    /// One of anvil's default dev accounts
    const LOCAL_KEY: &str =
        "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    fn network(chain_id: u64, rpc: &str) -> NetworkConfig {
        let url = |url: &str| SecretUrl::new(url.parse().unwrap());

        NetworkConfig {
            chain_id,
            name: format!("chain {chain_id}"),
            http_rpcs: vec![url(rpc)],
            ws_rpc: url("wss://ws.example.com"),
        }
    }

    fn relayer(id: &str, api_key_bytes: &[u8]) -> RelayerConfig {
        RelayerConfig {
            id: id.to_string(),
            name: "relayer".to_string(),
            chain_id: 1,
            key: RelayerKey::Local(SecretString::new(LOCAL_KEY.to_string())),
            address: Address::ZERO,
            max_inflight_txs: 5,
            max_queued_txs: 20,
            gas_price_limits: vec![],
            enabled: true,
            api_key_hashes: api_key_bytes
                .iter()
                .map(|byte| B256::with_last_byte(*byte))
                .collect(),
        }
    }

    #[test]
    fn version_ignores_order_and_rpc_urls() {
        let version = ConfigVersion::new(
            1,
            &[
                network(1, "https://a.example.com"),
                network(2, "https://a.example.com"),
            ],
            &[relayer("a", &[1, 2]), relayer("b", &[])],
        );
        let reordered = ConfigVersion::new(
            1,
            &[
                network(2, "https://b.example.com"),
                network(1, "https://b.example.com"),
            ],
            &[relayer("b", &[]), relayer("a", &[2, 1, 2])],
        );

        assert_eq!(version, reordered);
    }

    #[test]
    fn version_covers_relayer_settings() {
        let networks = [network(1, "https://a.example.com")];
        let version = ConfigVersion::new(1, &networks, &[relayer("a", &[1])]);

        let changes = [
            RelayerConfig {
                enabled: false,
                ..relayer("a", &[1])
            },
            RelayerConfig {
                max_queued_txs: 21,
                ..relayer("a", &[1])
            },
            relayer("a", &[2]),
        ];
        for changed in changes {
            assert_ne!(version, ConfigVersion::new(1, &networks, &[changed]));
        }
    }

    fn version(revision: u64, hash_byte: u8) -> Option<ConfigVersion> {
        Some(ConfigVersion {
            revision,
            hash: B256::with_last_byte(hash_byte),
        })
    }

    #[test_case(None, version(1, 1), Plan::Apply ; "nothing stored")]
    #[test_case(version(1, 1), None, Plan::Apply ; "no revision")]
    #[test_case(version(1, 1), version(2, 2), Plan::Apply ; "higher revision")]
    #[test_case(version(1, 1), version(1, 1), Plan::Skip ; "same revision and hash")]
    fn plans(
        stored: Option<ConfigVersion>,
        incoming: Option<ConfigVersion>,
        expected: Plan,
    ) {
        assert_eq!(plan(stored, incoming).unwrap(), expected);
    }

    #[test_case(
        version(2, 1),
        version(1, 1),
        "The config has revision 1, but the database applied revision 2 last" ;
        "lower revision"
    )]
    #[test_case(
        version(1, 1),
        version(1, 2),
        "The config has revision 1, which the database applied with other networks or relayers" ;
        "same revision with another hash"
    )]
    fn refuses(
        stored: Option<ConfigVersion>,
        incoming: Option<ConfigVersion>,
        expected_error: &str,
    ) {
        let err = plan(stored, incoming).unwrap_err();

        assert!(err.to_string().starts_with(expected_error), "{err}");
    }

    /// Fails its first `failures` loads with `error`, then loads local keys.
    struct FlakyKeys {
        failures: u32,
        error: fn() -> eyre::Report,
        attempts: AtomicU32,
    }

    #[async_trait::async_trait]
    impl KeysSource for FlakyKeys {
        async fn load_signer(
            &self,
            id: String,
        ) -> eyre::Result<UniversalSigner> {
            if self.attempts.fetch_add(1, AtomicOrdering::SeqCst)
                < self.failures
            {
                return Err((self.error)());
            }

            LocalKeys::new(&LocalKeysConfig::default())
                .load_signer(id)
                .await
        }
    }

    fn kms_timeout() -> eyre::Report {
        AwsSignerError::GetPublicKey(SdkError::timeout_error("timed out"))
            .into()
    }

    fn not_found() -> eyre::Report {
        eyre::eyre!("Key not found")
    }

    #[test_case(2, kms_timeout, true, 3 ; "transient errors are retried")]
    #[test_case(10, kms_timeout, false, KEY_LOAD_MAX_ATTEMPTS ; "retries are bounded")]
    #[test_case(1, not_found, false, 1 ; "other errors are not retried")]
    #[tokio::test(start_paused = true)]
    async fn key_loads_retry_transient_errors(
        failures: u32,
        error: fn() -> eyre::Report,
        succeeds: bool,
        expected_attempts: u32,
    ) {
        let keys = FlakyKeys {
            failures,
            error,
            attempts: AtomicU32::new(0),
        };

        let result = load_signer_with_retries(&keys, &relayer("a", &[])).await;

        assert_eq!(result.is_ok(), succeeds, "{result:?}");
        assert_eq!(
            keys.attempts.load(AtomicOrdering::SeqCst),
            expected_attempts
        );
    }
}
