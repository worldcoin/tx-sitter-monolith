//! Helpers for tests that run tx-sitter against misbehaving dependencies.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use postgres_docker_utils::DockerContainerGuard;
use tokio::net::TcpListener;

use crate::app::App;
use crate::config::{
    Config, DatabaseConfig, KeysConfig, LocalKeysConfig, ServerConfig,
    TxSitterConfig,
};

pub const RPC_TIMEOUT: Duration = Duration::from_secs(1);
pub const BLOCK_STREAM_TIMEOUT: Duration = Duration::from_secs(1);

/// The chain id of the network [`setup_app`] creates.
pub const CHAIN_ID: u64 = 1;

/// A TCP server that accepts connections and never answers.
pub struct UnresponsiveServer {
    pub addr: SocketAddr,
}

impl UnresponsiveServer {
    pub async fn spawn() -> eyre::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;

        tokio::spawn(async move {
            let mut connections = Vec::new();

            while let Ok((stream, _)) = listener.accept().await {
                connections.push(stream);
            }
        });

        Ok(Self { addr })
    }
}

/// Starts an app with local keys, `RPC_TIMEOUT` and `BLOCK_STREAM_TIMEOUT`,
/// and a network `CHAIN_ID` that uses the given RPC URLs.
pub async fn setup_app(
    http_rpc: &str,
    ws_rpc: &str,
) -> eyre::Result<(App, DockerContainerGuard)> {
    let db_container = postgres_docker_utils::setup().await?;
    let db_url = format!(
        "postgres://postgres:postgres@{}/database",
        db_container.address()
    );

    let config = Config {
        service: TxSitterConfig {
            escalation_interval: Duration::from_secs(60),
            max_escalations: 100,
            soft_reorg_interval: Duration::from_secs(60),
            hard_reorg_interval: Duration::from_secs(60),
            block_stream_timeout: BLOCK_STREAM_TIMEOUT,
            rpc_timeout: RPC_TIMEOUT,
            predefined: None,
            telemetry: None,
        },
        server: ServerConfig {
            host: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            username: None,
            password: None,
            server_address: None,
        },
        database: DatabaseConfig::connection_string(db_url),
        keys: KeysConfig::Local(LocalKeysConfig::default()),
    };

    let app = init_app(config).await?;
    app.db
        .upsert_network(CHAIN_ID, "test", http_rpc, ws_rpc)
        .await?;

    Ok((app, db_container))
}

async fn init_app(config: Config) -> eyre::Result<App> {
    for _ in 0..5 {
        match App::new(config.clone()).await {
            Ok(app) => return Ok(app),
            Err(err) => {
                eprintln!("Failed to initialize the app: {err:?}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }

    Err(eyre::eyre!("Failed to initialize the app"))
}
