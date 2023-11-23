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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ServerConfig {
    pub host: SocketAddr,

    #[serde(default)]
    pub disable_auth: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DatabaseConfig {
    pub connection_string: String,
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
    pub region: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LocalKeysConfig {}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::*;

    const SAMPLE: &str = indoc! {r#"
        [service]
        escalation_interval = "1h"

        [server]
        host = "127.0.0.1:3000"
        disable_auth = false

        [database]
        connection_string = "postgres://postgres:postgres@127.0.0.1:52804/database"

        [keys]
        kind = "local"
    "#};

    #[test]
    fn sample() {
        let config = Config {
            service: TxSitterConfig {
                escalation_interval: Duration::from_secs(60 * 60),
            },
            server: ServerConfig {
                host: SocketAddr::from(([127, 0, 0, 1], 3000)),
                disable_auth: false,
            },
            database: DatabaseConfig {
                connection_string:
                    "postgres://postgres:postgres@127.0.0.1:52804/database"
                        .to_string(),
            },
            keys: KeysConfig::Local(LocalKeysConfig {}),
        };

        let toml = toml::to_string_pretty(&config).unwrap();

        assert_eq!(toml, SAMPLE);
    }
}
