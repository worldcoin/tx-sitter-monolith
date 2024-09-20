use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use ethers::utils::AnvilInstance;
use tx_sitter::api_key::ApiKey;
use tx_sitter::config::{
    Config, DatabaseConfig, KeysConfig, LocalKeysConfig, Predefined,
    PredefinedNetwork, PredefinedRelayer, ServerConfig, TxSitterConfig,
};
use tx_sitter::service::Service;
use tx_sitter_client::apis::configuration::Configuration;

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
            escalation_interval: Duration::from_secs(5),
            soft_reorg_interval: Duration::from_secs(10),
            hard_reorg_interval: Duration::from_secs(15),
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
    ) -> eyre::Result<(Service, Configuration)> {
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
                server_address: None,
            },
            database: DatabaseConfig::connection_string(db_url),
            keys: KeysConfig::Local(LocalKeysConfig::default()),
        };

        let service = Service::new(config).await?;

        // Awaits for estimates to be ready
        let mut are_estimates_ready = false;
        for _ in 0..30 {
            if service
                .are_estimates_ready_for_chain(DEFAULT_ANVIL_CHAIN_ID)
                .await
            {
                are_estimates_ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        if !are_estimates_ready {
            eyre::bail!("Estimates were not ready!");
        }

        let client_config =
            tx_sitter_client::apis::configuration::ConfigurationBuilder::new()
                .base_path(format!("http://{}", service.local_addr()))
                .basic_auth("".to_string(), Some("".to_string()))
                .build();

        Ok((service, client_config))
    }
}
