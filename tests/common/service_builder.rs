use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use alloy::node_bindings::AnvilInstance;
use tokio_util::sync::CancellationToken;
use tx_sitter::api_key::ApiKey;
use tx_sitter::config::{
    Config, DatabaseConfig, KeysConfig, LocalKeysConfig, Predefined,
    PredefinedNetwork, PredefinedRelayer, ServerConfig, ServiceRole,
    TxSitterConfig,
};
use tx_sitter::service::Service;
use tx_sitter_client::apis::configuration::Configuration;

use super::await_estimates;
use super::prelude::{
    DEFAULT_ANVIL_CHAIN_ID, DEFAULT_ANVIL_PRIVATE_KEY, DEFAULT_RELAYER_ID,
};

pub struct ServiceBuilder {
    role: ServiceRole,
    escalation_interval: Duration,
    soft_reorg_interval: Duration,
    hard_reorg_interval: Duration,
    max_escalations: usize,
    rpc_timeout: Duration,
    task_stall_timeout: Duration,
    tx_retention: Duration,
    shutdown_timeout: Duration,
    shutdown: CancellationToken,
    inserts_predefined_values: bool,
}

impl Default for ServiceBuilder {
    fn default() -> Self {
        Self {
            role: ServiceRole::All,
            escalation_interval: Duration::from_secs(5),
            soft_reorg_interval: Duration::from_secs(10),
            hard_reorg_interval: Duration::from_secs(15),
            max_escalations: 100,
            rpc_timeout: Duration::from_secs(10),
            task_stall_timeout: Duration::from_secs(60),
            tx_retention: Duration::from_secs(30 * 24 * 60 * 60),
            shutdown_timeout: Duration::from_secs(20),
            shutdown: CancellationToken::new(),
            inserts_predefined_values: true,
        }
    }
}

impl ServiceBuilder {
    pub fn role(mut self, role: ServiceRole) -> Self {
        self.role = role;
        self
    }

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

    pub fn max_escalations(mut self, max: usize) -> Self {
        self.max_escalations = max;
        self
    }

    pub fn rpc_timeout(mut self, timeout: Duration) -> Self {
        self.rpc_timeout = timeout;
        self
    }

    pub fn task_stall_timeout(mut self, timeout: Duration) -> Self {
        self.task_stall_timeout = timeout;
        self
    }

    pub fn tx_retention(mut self, retention: Duration) -> Self {
        self.tx_retention = retention;
        self
    }

    pub fn shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }

    pub fn shutdown(mut self, shutdown: CancellationToken) -> Self {
        self.shutdown = shutdown;
        self
    }

    /// Starts the service without inserting the anvil network, relayer and
    /// API key into the database.
    pub fn without_predefined_values(mut self) -> Self {
        self.inserts_predefined_values = false;
        self
    }

    /// Starts the service and, if it indexes the predefined anvil network,
    /// waits until fee estimates for it are ready.
    pub async fn build(
        self,
        anvil: &AnvilInstance,
        db_url: &str,
    ) -> eyre::Result<(Service, Configuration)> {
        let anvil_private_key = hex::encode(DEFAULT_ANVIL_PRIVATE_KEY);

        let predefined = self.inserts_predefined_values.then(|| Predefined {
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
        });

        let config = Config {
            service: TxSitterConfig {
                role: self.role,
                escalation_interval: self.escalation_interval,
                max_escalations: self.max_escalations,
                soft_reorg_interval: self.soft_reorg_interval,
                hard_reorg_interval: self.hard_reorg_interval,
                block_stream_timeout: Duration::from_secs(60),
                rpc_timeout: self.rpc_timeout,
                task_stall_timeout: self.task_stall_timeout,
                tx_retention: self.tx_retention,
                event_retention: Duration::from_secs(7 * 24 * 60 * 60),
                telemetry: None,
                predefined,
            },
            server: ServerConfig {
                host: SocketAddr::V4(SocketAddrV4::new(
                    Ipv4Addr::new(127, 0, 0, 1),
                    0,
                )),
                shutdown_timeout: self.shutdown_timeout,
                username: None,
                password: None,
                server_address: None,
            },
            database: DatabaseConfig::connection_string(db_url),
            keys: KeysConfig::Local(LocalKeysConfig::default()),
        };

        let service = Service::new(config, self.shutdown).await?;

        if self.role.runs_tasks() && self.inserts_predefined_values {
            await_estimates(
                &service,
                DEFAULT_ANVIL_CHAIN_ID,
                Duration::from_secs(30),
            )
            .await?;
        }

        let client_config =
            tx_sitter_client::apis::configuration::ConfigurationBuilder::new()
                .base_path(format!("http://{}", service.local_addr()))
                .basic_auth("".to_string(), Some("".to_string()))
                .build();

        Ok((service, client_config))
    }
}
