use std::collections::HashMap;

use alloy::primitives::B256;
use eyre::Context;
use serde::Serialize;

use crate::config::{
    GasPriceLimitConfig, KeysConfig, RelayerConfig, RelayerKey,
};
use crate::db::data::{NetworkInfo, RelayerInfo};
use crate::db::Database;
use crate::types::secret_string::SecretString;

/// The networks, relayers and API keys of a database in the static config
/// format, see [`dump`].
///
/// It has no `Debug`, because `secrets` holds RPC URLs and private keys.
pub struct ConfigDump {
    /// The `[[networks]]` and `[[relayers]]` sections, which reference every
    /// secret as a `${NAME}` environment variable
    pub toml: String,

    /// The name and value of every environment variable `toml` references
    pub secrets: Vec<(String, String)>,
}

/// Reads the networks, relayers and API key hashes in `db` and writes them
/// in the static config format.
///
/// RPC URLs, and the private keys of local relayers, are secrets, so the TOML
/// references them as environment variables and [`ConfigDump::secrets`] lists
/// their values. Relayers of `keys.kind = "kms"` get `key.kms` and those of
/// `keys.kind = "local"` get `key.local`.
///
/// Loading the TOML, with the secrets in the environment, and applying it
/// with [`Database::apply_static_config`] leaves `db` as it is.
pub async fn dump(
    db: &Database,
    keys: &KeysConfig,
) -> eyre::Result<ConfigDump> {
    let networks = db.get_networks().await?;
    let relayers = db.get_relayers().await?;
    let api_key_hashes = db.get_api_key_hashes().await?;

    to_static_config(networks, relayers, api_key_hashes, keys)
}

/// A network as it appears in the dumped config, with RPC URLs replaced by
/// environment variable references
#[derive(Serialize)]
struct DumpedNetwork {
    chain_id: u64,
    name: String,
    http_rpcs: Vec<String>,
    ws_rpc: String,
}

#[derive(Serialize)]
struct DumpedConfig {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    networks: Vec<DumpedNetwork>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    relayers: Vec<RelayerConfig>,
}

fn to_static_config(
    mut networks: Vec<NetworkInfo>,
    mut relayers: Vec<RelayerInfo>,
    api_key_hashes: Vec<(String, B256)>,
    keys: &KeysConfig,
) -> eyre::Result<ConfigDump> {
    let mut secrets = vec![];
    let mut secret = |name: String, value: String| {
        let reference = format!("${{{name}}}");
        secrets.push((name, value));

        reference
    };

    networks.sort_by_key(|network| network.chain_id);
    let networks = networks
        .into_iter()
        .map(|network| DumpedNetwork {
            chain_id: network.chain_id,
            name: network.name,
            http_rpcs: vec![secret(
                format!("HTTP_RPC_{}", network.chain_id),
                network.http_rpc,
            )],
            ws_rpc: secret(
                format!("WS_RPC_{}", network.chain_id),
                network.ws_rpc,
            ),
        })
        .collect();

    let mut api_key_hashes_by_relayer: HashMap<String, Vec<B256>> =
        HashMap::new();
    for (relayer_id, hash) in api_key_hashes {
        api_key_hashes_by_relayer
            .entry(relayer_id)
            .or_default()
            .push(hash);
    }

    relayers.sort_by(|a, b| {
        (a.chain_id, &a.name, &a.id).cmp(&(b.chain_id, &b.name, &b.id))
    });
    let relayers = relayers
        .into_iter()
        .map(|relayer| {
            let key = match keys {
                KeysConfig::Kms(_) => RelayerKey::Kms(relayer.key_id),
                KeysConfig::Local(_) => {
                    let name = format!(
                        "RELAYER_KEY_{}",
                        relayer.id.to_uppercase().replace('-', "_")
                    );

                    RelayerKey::Local(SecretString::new(secret(name, relayer.key_id)))
                }
            };

            let gas_price_limits = relayer
                .gas_price_limits
                .into_iter()
                .map(|limit| {
                    Ok(GasPriceLimitConfig {
                        chain_id: limit.chain_id.try_into().with_context(|| {
                            format!(
                                "Relayer {} has a gas price limit for chain id {}",
                                relayer.id, limit.chain_id
                            )
                        })?,
                        value: limit.value.into(),
                    })
                })
                .collect::<eyre::Result<_>>()?;

            Ok(RelayerConfig {
                api_key_hashes: api_key_hashes_by_relayer
                    .remove(&relayer.id)
                    .unwrap_or_default(),
                id: relayer.id,
                name: relayer.name,
                chain_id: relayer.chain_id,
                key,
                address: relayer.address.0,
                max_inflight_txs: relayer.max_inflight_txs,
                max_queued_txs: relayer.max_queued_txs,
                gas_price_limits,
                enabled: relayer.enabled,
            })
        })
        .collect::<eyre::Result<_>>()?;

    let toml = toml::to_string(&DumpedConfig { networks, relayers })?;

    Ok(ConfigDump { toml, secrets })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use alloy::primitives::{address, U256};
    use indoc::indoc;

    use super::*;
    use crate::config::{load_config, KmsKeysConfig, LocalKeysConfig};
    use crate::db::data::{Reconciliation, RelayerGasPriceLimit};
    use crate::db::wrappers::address::AddressWrapper;
    use crate::test_utils::setup_app;

    const RELAYER_ID: &str = "6b0b6d1c-7b0e-4c2a-9d3e-2f1a5c4b3a29";
    const OTHER_RELAYER_ID: &str = "1b908a34-5dc1-4d2d-a146-5eb46e975830";

    fn kms_keys() -> KeysConfig {
        KeysConfig::Kms(KmsKeysConfig {
            kms_timeout: Duration::from_secs(5),
        })
    }

    fn network(chain_id: u64) -> NetworkInfo {
        NetworkInfo {
            chain_id,
            name: format!("chain \"{chain_id}\""),
            http_rpc: format!("https://rpc.example.com/{chain_id}/key"),
            ws_rpc: format!("wss://ws.example.com/{chain_id}/key"),
        }
    }

    fn relayer(id: &str, chain_id: u64, name: &str) -> RelayerInfo {
        RelayerInfo {
            id: id.to_string(),
            name: name.to_string(),
            chain_id,
            key_id: format!("key-{name}"),
            address: AddressWrapper(address!(
                "1Ed53d680B8890DAe2a63f673a85fFDE1FD5C7a2"
            )),
            nonce: 7,
            current_nonce: 5,
            max_inflight_txs: 5,
            max_queued_txs: 20,
            gas_price_limits: vec![],
            enabled: true,
        }
    }

    #[test]
    fn dumps_networks_and_relayers_sorted_with_secrets_as_env_vars() {
        let dump = to_static_config(
            vec![network(480), network(10)],
            vec![
                RelayerInfo {
                    gas_price_limits: vec![RelayerGasPriceLimit {
                        value: U256::from(100_000_000_000u64).into(),
                        chain_id: 480,
                    }],
                    enabled: false,
                    ..relayer(RELAYER_ID, 480, "b")
                },
                relayer(OTHER_RELAYER_ID, 480, "a"),
            ],
            vec![
                (RELAYER_ID.to_string(), B256::with_last_byte(1)),
                (RELAYER_ID.to_string(), B256::with_last_byte(2)),
            ],
            &kms_keys(),
        )
        .unwrap();

        assert_eq!(
            dump.toml,
            indoc! {r#"
                [[networks]]
                chain_id = 10
                name = "chain \"10\""
                http_rpcs = ["${HTTP_RPC_10}"]
                ws_rpc = "${WS_RPC_10}"

                [[networks]]
                chain_id = 480
                name = "chain \"480\""
                http_rpcs = ["${HTTP_RPC_480}"]
                ws_rpc = "${WS_RPC_480}"

                [[relayers]]
                id = "1b908a34-5dc1-4d2d-a146-5eb46e975830"
                name = "a"
                chain_id = 480
                address = "0x1Ed53d680B8890DAe2a63f673a85fFDE1FD5C7a2"
                max_inflight_txs = 5
                max_queued_txs = 20
                gas_price_limits = []
                enabled = true
                api_key_hashes = []

                [relayers.key]
                kms = "key-a"

                [[relayers]]
                id = "6b0b6d1c-7b0e-4c2a-9d3e-2f1a5c4b3a29"
                name = "b"
                chain_id = 480
                address = "0x1Ed53d680B8890DAe2a63f673a85fFDE1FD5C7a2"
                max_inflight_txs = 5
                max_queued_txs = 20
                enabled = false
                api_key_hashes = ["0x0000000000000000000000000000000000000000000000000000000000000001", "0x0000000000000000000000000000000000000000000000000000000000000002"]

                [relayers.key]
                kms = "key-b"

                [[relayers.gas_price_limits]]
                chain_id = 480
                value = "100000000000"
            "#}
        );
        assert_eq!(
            dump.secrets,
            [
                ("HTTP_RPC_10", "https://rpc.example.com/10/key"),
                ("WS_RPC_10", "wss://ws.example.com/10/key"),
                ("HTTP_RPC_480", "https://rpc.example.com/480/key"),
                ("WS_RPC_480", "wss://ws.example.com/480/key"),
            ]
            .map(|(name, value)| (name.to_string(), value.to_string()))
        );
    }

    #[test]
    fn dumps_nothing_for_an_empty_database() {
        let dump =
            to_static_config(vec![], vec![], vec![], &kms_keys()).unwrap();

        assert_eq!(dump.toml, "");
        assert_eq!(dump.secrets, vec![]);
    }

    #[test]
    fn dumps_local_keys_as_env_vars() {
        let dump = to_static_config(
            vec![network(480)],
            vec![relayer(RELAYER_ID, 480, "a")],
            vec![],
            &KeysConfig::Local(LocalKeysConfig::default()),
        )
        .unwrap();

        assert!(
            dump.toml.contains(
                "[relayers.key]\nlocal = \"${RELAYER_KEY_6B0B6D1C_7B0E_4C2A_9D3E_2F1A5C4B3A29}\""
            ),
            "{}",
            dump.toml
        );
        assert!(dump.secrets.contains(&(
            "RELAYER_KEY_6B0B6D1C_7B0E_4C2A_9D3E_2F1A5C4B3A29".to_string(),
            "key-a".to_string()
        )));
    }

    /// The state the admin API leaves behind: networks, enabled and disabled
    /// relayers with nonces and gas price limits, and API keys
    async fn insert_admin_api_state(db: &Database) -> eyre::Result<()> {
        sqlx::Executor::execute(
            &db.pool,
            r#"
            INSERT INTO networks (chain_id, name) VALUES (7480, 'worldchain'), (7010, 'optimism');
            INSERT INTO rpcs (chain_id, url, kind) VALUES
                (7480, 'https://worldchain.example.com/v2/secret', 'http'),
                (7480, 'wss://worldchain.example.com/v2/secret', 'ws'),
                (7010, 'https://optimism.example.com/v2/secret', 'http'),
                (7010, 'wss://optimism.example.com/v2/secret', 'ws');
            INSERT INTO relayers (
                id, name, chain_id, key_id, address, nonce, current_nonce,
                max_inflight_txs, max_queued_txs, gas_price_limits, enabled
            ) VALUES
                ('6b0b6d1c-7b0e-4c2a-9d3e-2f1a5c4b3a29', 'wc_enableSafe4337Module_1', 7480, 'kms-key-1',
                 '\x1ed53d680b8890dae2a63f673a85ffde1fd5c7a2', 12, 10, 10, 50,
                 '[{"value": "100000000000", "chainId": 7480}]', TRUE),
                ('1b908a34-5dc1-4d2d-a146-5eb46e975830', 'retired', 7010, 'kms-key-2',
                 '\xf39fd6e51aad88f6f4ce6ab8827279cfffb92266', 3, 3, 5, 20, '[]', FALSE);
            INSERT INTO api_keys (relayer_id, key_hash) VALUES
                ('6b0b6d1c-7b0e-4c2a-9d3e-2f1a5c4b3a29', '\x0101010101010101010101010101010101010101010101010101010101010101'),
                ('6b0b6d1c-7b0e-4c2a-9d3e-2f1a5c4b3a29', '\x0202020202020202020202020202020202020202020202020202020202020202'),
                ('1b908a34-5dc1-4d2d-a146-5eb46e975830', '\x0303030303030303030303030303030303030303030303030303030303030303');
            "#,
        )
        .await?;

        Ok(())
    }

    fn temp_config_file(toml: &str) -> eyre::Result<std::path::PathBuf> {
        let path = std::env::temp_dir()
            .join(format!("tx-sitter-dump-{}.toml", uuid::Uuid::new_v4()));
        std::fs::write(&path, toml)?;

        Ok(path)
    }

    /// Loads the dump after a base config, as the README tells operators to
    #[tokio::test]
    async fn applying_a_dump_leaves_the_database_as_it_is() -> eyre::Result<()>
    {
        let (app, _db_container) =
            setup_app("http://127.0.0.1:1", "ws://127.0.0.1:1").await?;
        let db = &app.db;
        insert_admin_api_state(db).await?;
        let relayers_before = db.get_relayers().await?;
        let networks_before = db.get_networks().await?;
        let api_key_hashes_before = db.get_api_key_hashes().await?;

        let read_only_db =
            Database::connect_read_only(&app.config.database).await?;
        let dump = dump(&read_only_db, &kms_keys()).await?;
        for (name, value) in &dump.secrets {
            std::env::set_var(name, value);
        }
        let base_path = temp_config_file(indoc! {r#"
            [service]
            escalation_interval = "1m"

            [server]
            host = "127.0.0.1:3000"

            [database]
            kind = "connection_string"
            connection_string = "postgres://unused"

            [keys]
            kind = "kms"
        "#})?;
        let dump_path = temp_config_file(&dump.toml)?;
        let config =
            load_config([base_path.as_path(), dump_path.as_path()].into_iter());
        std::fs::remove_file(&base_path)?;
        std::fs::remove_file(&dump_path)?;
        let config = config?;
        config.validate()?;

        let reconciliation = db
            .apply_static_config(&config.networks, &config.relayers, None)
            .await?;

        assert_eq!(reconciliation, Reconciliation::Applied(vec![]));
        assert_eq!(db.get_relayers().await?, relayers_before);
        assert_eq!(db.get_networks().await?, networks_before);
        assert_eq!(db.get_api_key_hashes().await?, api_key_hashes_before);
        assert_eq!(super::dump(db, &kms_keys()).await?.toml, dump.toml);
        assert_eq!(relayers_before.len(), 2);
        assert_eq!(api_key_hashes_before.len(), 3);

        Ok(())
    }
}
