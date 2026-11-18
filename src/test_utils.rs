//! Helpers for tests that run tx-sitter against misbehaving dependencies.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use poem::listener::{Acceptor, Listener};
use postgres_docker_utils::DockerContainerGuard;
use tokio::net::TcpListener;

use crate::app::App;
use crate::config::{
    Config, DatabaseConfig, KeysConfig, LocalKeysConfig, NetworkConfig,
    ServerConfig, ServiceRole, TxSitterConfig,
};
use crate::types::secret_url::SecretUrl;

pub const RPC_TIMEOUT: Duration = Duration::from_secs(1);
pub const BLOCK_STREAM_TIMEOUT: Duration = Duration::from_secs(1);

/// The chain id of the network [`setup_app`] creates.
pub const CHAIN_ID: u64 = 1;

/// A TCP server that accepts connections and never answers.
pub struct UnresponsiveServer {
    pub addr: SocketAddr,
    accepted_connections: Arc<AtomicUsize>,
}

impl UnresponsiveServer {
    pub async fn spawn() -> eyre::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;
        let accepted_connections = Arc::new(AtomicUsize::new(0));

        let counter = accepted_connections.clone();
        tokio::spawn(async move {
            let mut connections = Vec::new();

            while let Ok((stream, _)) = listener.accept().await {
                counter.fetch_add(1, Ordering::SeqCst);
                connections.push(stream);
            }
        });

        Ok(Self {
            addr,
            accepted_connections,
        })
    }

    /// Returns how many connections the server has accepted so far.
    pub fn accepted_connections(&self) -> usize {
        self.accepted_connections.load(Ordering::SeqCst)
    }
}

/// An HTTP JSON-RPC server that answers every request the same way.
pub struct StubRpc {
    pub url: String,
    requests: Arc<AtomicUsize>,
}

impl StubRpc {
    /// Starts a server that answers with `status` and a JSON-RPC response
    /// with the request's id and `payload`, such as `{"result": "0x2a"}`.
    ///
    /// The server's URL ends in `/v2/secret-key`, like a provider URL.
    pub async fn spawn(
        status: poem::http::StatusCode,
        payload: serde_json::Value,
    ) -> eyre::Result<Self> {
        let acceptor = poem::listener::TcpListener::bind("127.0.0.1:0")
            .into_acceptor()
            .await?;
        let addr = acceptor.local_addr()[0]
            .as_socket_addr()
            .copied()
            .ok_or_else(|| eyre::eyre!("Stub RPC has no socket address"))?;
        let requests = Arc::new(AtomicUsize::new(0));

        let counter = requests.clone();
        let answer = poem::endpoint::make(move |request: poem::Request| {
            let counter = counter.clone();
            let payload = payload.clone();

            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                let body: serde_json::Value =
                    request.into_body().into_json().await?;

                let mut response = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": body["id"],
                });
                response
                    .as_object_mut()
                    .expect("Response is an object")
                    .extend(payload.as_object().cloned().unwrap_or_default());

                Ok::<_, poem::Error>(
                    poem::Response::builder()
                        .status(status)
                        .content_type("application/json")
                        .body(response.to_string()),
                )
            }
        });
        tokio::spawn(poem::Server::new_with_acceptor(acceptor).run(answer));

        Ok(Self {
            url: format!("http://{addr}/v2/secret-key"),
            requests,
        })
    }

    /// Returns how many requests the server has answered so far.
    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
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
            role: ServiceRole::All,
            config_revision: None,
            escalation_interval: Duration::from_secs(60),
            max_escalations: 100,
            soft_reorg_interval: Duration::from_secs(60),
            hard_reorg_interval: Duration::from_secs(60),
            block_stream_timeout: BLOCK_STREAM_TIMEOUT,
            rpc_timeout: RPC_TIMEOUT,
            task_stall_timeout: Duration::from_secs(60),
            tx_retention: Duration::from_secs(30 * 24 * 60 * 60),
            event_retention: Duration::from_secs(7 * 24 * 60 * 60),
            telemetry: None,
        },
        server: ServerConfig {
            host: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            shutdown_timeout: Duration::from_secs(20),
            max_event_waiters: 64,
            server_address: None,
        },
        database: DatabaseConfig::connection_string(db_url),
        keys: KeysConfig::Local(LocalKeysConfig::default()),
        networks: vec![NetworkConfig {
            chain_id: CHAIN_ID,
            name: "test".to_string(),
            http_rpcs: vec![SecretUrl::new(http_rpc.parse()?)],
            ws_rpc: SecretUrl::new(ws_rpc.parse()?),
        }],
        relayers: vec![],
    };

    let app = init_app(config).await?;

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
