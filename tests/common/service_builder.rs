use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use alloy::node_bindings::AnvilInstance;
use tokio_util::sync::CancellationToken;
use tx_sitter::config::{
    Config, DatabaseConfig, KeysConfig, LocalKeysConfig, NetworkConfig,
    RelayerConfig, ServerConfig, ServiceRole, TxSitterConfig,
};
use tx_sitter::service::Service;
use tx_sitter_client::apis::configuration::Configuration;

use super::prelude::DEFAULT_ANVIL_CHAIN_ID;
use super::{anvil_network, await_estimates, default_relayer};

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
    max_event_waiters: usize,
    shutdown: CancellationToken,
    uses_defaults: bool,
    networks: Vec<NetworkConfig>,
    relayers: Vec<RelayerConfig>,
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
            max_event_waiters: 64,
            shutdown: CancellationToken::new(),
            uses_defaults: true,
            networks: vec![],
            relayers: vec![],
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

    pub fn max_event_waiters(mut self, max: usize) -> Self {
        self.max_event_waiters = max;
        self
    }

    pub fn shutdown(mut self, shutdown: CancellationToken) -> Self {
        self.shutdown = shutdown;
        self
    }

    /// Configures the service without the anvil network and the default
    /// relayer.
    pub fn without_defaults(mut self) -> Self {
        self.uses_defaults = false;
        self
    }

    /// Adds `network` to the configured networks.
    pub fn network(mut self, network: NetworkConfig) -> Self {
        self.networks.push(network);
        self
    }

    /// Adds `relayer` to the configured relayers.
    pub fn relayer(mut self, relayer: RelayerConfig) -> Self {
        self.relayers.push(relayer);
        self
    }

    /// Starts the service with the anvil network and the default relayer,
    /// unless [`Self::without_defaults`] was called, next to the added
    /// networks and relayers.
    ///
    /// If the service indexes the anvil network, this waits until fee
    /// estimates for it are ready.
    pub async fn build(
        self,
        anvil: &AnvilInstance,
        db_url: &str,
    ) -> eyre::Result<(Service, Configuration)> {
        let (mut networks, mut relayers) = if self.uses_defaults {
            (vec![anvil_network(anvil)], vec![default_relayer()])
        } else {
            (vec![], vec![])
        };
        networks.extend(self.networks);
        relayers.extend(self.relayers);

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
                // The service refuses to keep events longer than txs
                event_retention: self
                    .tx_retention
                    .min(Duration::from_secs(7 * 24 * 60 * 60)),
                telemetry: None,
            },
            server: ServerConfig {
                host: SocketAddr::V4(SocketAddrV4::new(
                    Ipv4Addr::new(127, 0, 0, 1),
                    0,
                )),
                shutdown_timeout: self.shutdown_timeout,
                max_event_waiters: self.max_event_waiters,
                username: None,
                password: None,
                server_address: None,
            },
            database: DatabaseConfig::connection_string(db_url),
            keys: KeysConfig::Local(LocalKeysConfig::default()),
            networks,
            relayers,
        };

        let service = Service::new(config, self.shutdown).await?;

        if self.role.runs_tasks() && self.uses_defaults {
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
