use std::time::Duration;

use alloy::providers::{ProviderBuilder, RootProvider, WsConnect};
use alloy::rpc::client::RpcClient;
use eyre::Context;
use tokio::time::timeout;
use url::Url;

use crate::api_key::ApiKey;
use crate::config::{Config, KeysConfig};
use crate::db::data::RpcKind;
use crate::db::Database;
use crate::keys::{KeysSource, KmsKeys, LocalKeys, UniversalSigner};

const RPC_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct App {
    pub config: Config,

    pub keys_source: Box<dyn KeysSource>,

    pub db: Database,

    /// Client for HTTP RPC requests, shared by the providers and the RPC
    /// proxy, with requests bounded by `service.rpc_timeout`
    pub rpc_client: reqwest::Client,
}

impl App {
    pub async fn new(config: Config) -> eyre::Result<Self> {
        tracing::info!("Initializing keys source");
        let keys_source = init_keys_source(&config).await?;

        tracing::info!("Initializing db");
        let db = Database::new(&config.database).await?;

        let rpc_client = rpc_http_client(config.service.rpc_timeout)?;

        Ok(Self {
            config,
            keys_source,
            db,
            rpc_client,
        })
    }

    pub async fn http_rpc_url(&self, chain_id: u64) -> eyre::Result<Url> {
        let url = self.db.get_network_rpc(chain_id, RpcKind::Http).await?;

        url.parse().context("Invalid HTTP RPC URL")
    }

    pub async fn http_provider(
        &self,
        chain_id: u64,
    ) -> eyre::Result<RootProvider> {
        let url = self.http_rpc_url(chain_id).await?;

        let client =
            RpcClient::new_http_with_client(self.rpc_client.clone(), url);

        Ok(RootProvider::new(client))
    }

    pub async fn ws_provider(
        &self,
        chain_id: u64,
    ) -> eyre::Result<RootProvider> {
        let url = self.db.get_network_rpc(chain_id, RpcKind::Ws).await?;

        let provider = timeout(
            self.config.service.rpc_timeout,
            ProviderBuilder::default().connect_ws(WsConnect::new(url)),
        )
        .await
        .context("Timed out connecting to WS provider")?
        .context("Connecting to WS provider")?;

        Ok(provider)
    }

    pub async fn load_signer(
        &self,
        key_id: String,
    ) -> eyre::Result<UniversalSigner> {
        self.keys_source
            .load_signer(key_id)
            .await
            .context("Missing signer")
    }

    pub async fn is_authorized(
        &self,
        api_token: &ApiKey,
    ) -> eyre::Result<bool> {
        self.db
            .is_api_key_valid(
                api_token.relayer_id(),
                api_token.api_key_secret_hash(),
            )
            .await
    }
}

async fn init_keys_source(
    config: &Config,
) -> eyre::Result<Box<dyn KeysSource>> {
    let keys_source: Box<dyn KeysSource> = match &config.keys {
        KeysConfig::Kms(kms_config) => {
            Box::new(KmsKeys::new(kms_config).await?)
        }
        KeysConfig::Local(local_config) => {
            Box::new(LocalKeys::new(local_config))
        }
    };

    Ok(keys_source)
}

/// Builds the client for HTTP RPC requests.
///
/// It trusts only the Mozilla root certificates bundled into the binary, the
/// same roots as alloy's WS transport, which always uses its bundled copy.
fn rpc_http_client(rpc_timeout: Duration) -> eyre::Result<reqwest::Client> {
    let root_certs = webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .map(|cert| reqwest::Certificate::from_der(cert))
        .collect::<Result<Vec<_>, _>>()
        .context("Parsing bundled root certificates")?;

    reqwest::Client::builder()
        .tls_certs_only(root_certs)
        .connect_timeout(RPC_CONNECT_TIMEOUT)
        .timeout(rpc_timeout)
        .build()
        .context("Building RPC HTTP client")
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};
    use std::time::Instant;

    use alloy::providers::Provider;
    use postgres_docker_utils::DockerContainerGuard;
    use tokio::net::TcpListener;

    use super::*;
    use crate::config::{
        DatabaseConfig, LocalKeysConfig, ServerConfig, TxSitterConfig,
    };

    const RPC_TIMEOUT: Duration = Duration::from_secs(1);
    const CHAIN_ID: u64 = 1;

    #[tokio::test]
    async fn http_rpc_call_fails_within_rpc_timeout() -> eyre::Result<()> {
        let (app, _db_container) = setup_app_with_unresponsive_rpc().await?;
        let provider = app.http_provider(CHAIN_ID).await?;

        let started = Instant::now();
        let result = timeout(RPC_TIMEOUT * 3, provider.get_block_number())
            .await
            .context("RPC call outlived rpc_timeout")?;

        assert!(result.is_err(), "Unexpected response: {result:?}");
        assert!(started.elapsed() >= RPC_TIMEOUT);

        Ok(())
    }

    #[tokio::test]
    async fn ws_connect_fails_within_rpc_timeout() -> eyre::Result<()> {
        let (app, _db_container) = setup_app_with_unresponsive_rpc().await?;

        let started = Instant::now();
        let result = timeout(RPC_TIMEOUT * 3, app.ws_provider(CHAIN_ID))
            .await
            .context("WS connect outlived rpc_timeout")?;

        assert!(result.is_err(), "Unexpected WS connection");
        assert!(started.elapsed() >= RPC_TIMEOUT);

        Ok(())
    }

    async fn setup_app_with_unresponsive_rpc(
    ) -> eyre::Result<(App, DockerContainerGuard)> {
        let rpc_addr = spawn_unresponsive_rpc().await?;
        let (app, db_container) = setup_app().await?;

        app.db
            .upsert_network(
                CHAIN_ID,
                "unresponsive",
                &format!("http://{rpc_addr}"),
                &format!("ws://{rpc_addr}"),
            )
            .await?;

        Ok((app, db_container))
    }

    /// Starts a TCP server that accepts connections and never answers.
    async fn spawn_unresponsive_rpc() -> eyre::Result<SocketAddr> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;

        tokio::spawn(async move {
            let mut connections = Vec::new();

            while let Ok((stream, _)) = listener.accept().await {
                connections.push(stream);
            }
        });

        Ok(addr)
    }

    async fn setup_app() -> eyre::Result<(App, DockerContainerGuard)> {
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
                block_stream_timeout: Duration::from_secs(60),
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

        for _ in 0..5 {
            match App::new(config.clone()).await {
                Ok(app) => return Ok((app, db_container)),
                Err(err) => {
                    eprintln!("Failed to initialize the app: {err:?}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }

        Err(eyre::eyre!("Failed to initialize the app"))
    }
}
