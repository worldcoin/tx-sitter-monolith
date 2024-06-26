use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use ethers::utils::AnvilInstance;
use tx_sitter::api_key::ApiKey;
use tx_sitter::client::TxSitterClient;
use tx_sitter::config::{
    Config, DatabaseConfig, KeysConfig, LocalKeysConfig, Predefined,
    PredefinedNetwork, PredefinedRelayer, ServerConfig, TxSitterConfig,
};
use tx_sitter::service::Service;

use alloy::node_bindings::AnvilInstance as AlloyAnvilInstance;

use super::prelude::{
    DEFAULT_ANVIL_CHAIN_ID, DEFAULT_ANVIL_PRIVATE_KEY, DEFAULT_RELAYER_ID,
};

pub struct ServiceBuilder {
    escalation_interval: Duration,
    soft_reorg_interval: Duration,
    hard_reorg_interval: Duration,
}

impl Default for ServiceBuilder {
    fn default() -> Self {
        Self {
            escalation_interval: Duration::from_secs(30),
            soft_reorg_interval: Duration::from_secs(45),
            hard_reorg_interval: Duration::from_secs(60),
        }
    }
}

impl ServiceBuilder {
    pub fn escalation_interval(mut self, interval: Duration) -> Self {
        self.escalation_interval = interval;
        self
    }

    pub fn soft_reorg_interval(mut self, interval: Duration) -> Self {
        self.soft_reorg_interval = interval;
        self
    }

    pub fn hard_reorg_interval(mut self, interval: Duration) -> Self {
        self.hard_reorg_interval = interval;
        self
    }

    pub async fn build(
        self,
        anvil: &AnvilInstance,
        db_url: &str,
    ) -> eyre::Result<(Service, TxSitterClient)> {
        let anvil_private_key = hex::encode(DEFAULT_ANVIL_PRIVATE_KEY);

        let config = Config {
            service: TxSitterConfig {
                escalation_interval: self.escalation_interval,
                soft_reorg_interval: self.soft_reorg_interval,
                hard_reorg_interval: self.hard_reorg_interval,
                telemetry: None,
                predefined: Some(Predefined {
                    network: PredefinedNetwork {
                        chain_id: DEFAULT_ANVIL_CHAIN_ID,
                        name: "Anvil".to_string(),
                        http_rpc: anvil.endpoint(),
                        ws_rpc: anvil.ws_endpoint(),
                    },
                    relayer: PredefinedRelayer {
                        name: "Anvil".to_string(),
                        id: DEFAULT_RELAYER_ID.to_string(),
                        key_id: anvil_private_key,
                        chain_id: DEFAULT_ANVIL_CHAIN_ID,
                        // TODO: Use this key in tests
                        api_key: ApiKey::random(DEFAULT_RELAYER_ID),
                    },
                }),
            },
            server: ServerConfig {
                host: SocketAddr::V4(SocketAddrV4::new(
                    Ipv4Addr::new(127, 0, 0, 1),
                    0,
                )),
                username: None,
                password: None,
            },
            database: DatabaseConfig::connection_string(db_url),
            keys: KeysConfig::Local(LocalKeysConfig::default()),
        };

        let service = Service::new(config).await?;

        let client =
            TxSitterClient::new(format!("http://{}", service.local_addr()));

        Ok((service, client))
    }

    pub async fn build_for_alloy(
        self,
        anvil: &AlloyAnvilInstance,
        db_url: &str,
    ) -> eyre::Result<(Service, TxSitterClient)> {
        let anvil_private_key = hex::encode(anvil.keys()[1].to_bytes());

        let config = Config {
            service: TxSitterConfig {
                escalation_interval: self.escalation_interval,
                soft_reorg_interval: self.soft_reorg_interval,
                hard_reorg_interval: self.hard_reorg_interval,
                telemetry: None,
                predefined: Some(Predefined {
                    network: PredefinedNetwork {
                        chain_id: DEFAULT_ANVIL_CHAIN_ID,
                        name: "Anvil".to_string(),
                        http_rpc: anvil.endpoint(),
                        ws_rpc: anvil.ws_endpoint(),
                    },
                    relayer: PredefinedRelayer {
                        name: "Anvil".to_string(),
                        id: DEFAULT_RELAYER_ID.to_string(),
                        key_id: anvil_private_key,
                        chain_id: DEFAULT_ANVIL_CHAIN_ID,
                        // TODO: Use this key in tests
                        api_key: ApiKey::random(DEFAULT_RELAYER_ID),
                    },
                }),
            },
            server: ServerConfig {
                host: SocketAddr::V4(SocketAddrV4::new(
                    Ipv4Addr::new(127, 0, 0, 1),
                    0,
                )),
                username: None,
                password: None,
            },
            database: DatabaseConfig::connection_string(db_url),
            keys: KeysConfig::Local(LocalKeysConfig::default()),
        };

        let service = Service::new(config).await?;

        let client =
            TxSitterClient::new(format!("http://{}", service.local_addr()));

        Ok((service, client))
    }
}
