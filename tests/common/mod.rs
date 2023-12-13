#![allow(dead_code)] // Needed because this module is imported as module by many test crates

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use ethers::core::k256::ecdsa::SigningKey;
use ethers::middleware::SignerMiddleware;
use ethers::providers::{Http, Middleware, Provider};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::{Address, Eip1559TransactionRequest, H160, U256};
use fake_rpc::DoubleAnvil;
use postgres_docker_utils::DockerContainerGuard;
use tokio::task::JoinHandle;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use tx_sitter::client::TxSitterClient;
use tx_sitter::config::{
    Config, DatabaseConfig, KeysConfig, LocalKeysConfig, PredefinedNetwork,
    PredefinedRelayer, ServerConfig, TxSitterConfig,
};
use tx_sitter::service::Service;

pub type AppMiddleware = SignerMiddleware<Arc<Provider<Http>>, LocalWallet>;

#[allow(unused_imports)]
pub mod prelude {
    pub use std::time::Duration;

    pub use ethers::providers::Middleware;
    pub use ethers::types::{Eip1559TransactionRequest, U256};
    pub use ethers::utils::parse_units;
    pub use tx_sitter::server::routes::relayer::{
        CreateRelayerRequest, CreateRelayerResponse,
    };
    pub use tx_sitter::server::routes::transaction::SendTxRequest;

    pub use super::*;
}

pub const DEFAULT_ANVIL_ACCOUNT: Address = H160(hex_literal::hex!(
    "f39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
));

pub const DEFAULT_ANVIL_PRIVATE_KEY: &[u8] = &hex_literal::hex!(
    "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
);

pub const SECONDARY_ANVIL_PRIVATE_KEY: &[u8] = &hex_literal::hex!(
    "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
);

pub const ARBITRARY_ADDRESS: Address = H160(hex_literal::hex!(
    "1Ed53d680B8890DAe2a63f673a85fFDE1FD5C7a2"
));

pub const DEFAULT_ANVIL_CHAIN_ID: u64 = 31337;

pub const DEFAULT_RELAYER_ID: &str = "1b908a34-5dc1-4d2d-a146-5eb46e975830";

pub struct DoubleAnvilHandle {
    pub double_anvil: Arc<DoubleAnvil>,
    ws_addr: String,
    local_addr: SocketAddr,
    server_handle: JoinHandle<eyre::Result<()>>,
}

impl DoubleAnvilHandle {
    pub fn local_addr(&self) -> String {
        self.local_addr.to_string()
    }

    pub fn ws_addr(&self) -> String {
        self.ws_addr.clone()
    }
}

pub fn setup_tracing() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().pretty().compact())
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                // Logging from fake_rpc can get very messy so we set it to warn only
                .parse_lossy("info,fake_rpc=warn"),
        )
        .init();
}

pub async fn setup_db() -> eyre::Result<(String, DockerContainerGuard)> {
    let db_container = postgres_docker_utils::setup().await?;
    let db_socket_addr = db_container.address();
    let url = format!("postgres://postgres:postgres@{db_socket_addr}/database");

    Ok((url, db_container))
}

pub async fn setup_double_anvil() -> eyre::Result<DoubleAnvilHandle> {
    let (double_anvil, server) = fake_rpc::serve(0).await;

    let local_addr = server.local_addr();

    let server_handle = tokio::spawn(async move {
        server.await?;
        Ok(())
    });

    let middleware = setup_middleware(
        format!("http://{local_addr}"),
        SECONDARY_ANVIL_PRIVATE_KEY,
    )
    .await?;

    // We need to seed some transactions so we can get fee estimates on the first block
    middleware
        .send_transaction(
            Eip1559TransactionRequest {
                to: Some(DEFAULT_ANVIL_ACCOUNT.into()),
                value: Some(U256::from(100u64)),
                ..Default::default()
            },
            None,
        )
        .await?
        .await?;

    let ws_addr = double_anvil.ws_endpoint().await;

    Ok(DoubleAnvilHandle {
        double_anvil,
        ws_addr,
        local_addr,
        server_handle,
    })
}

pub async fn setup_service(
    anvil_handle: &DoubleAnvilHandle,
    db_connection_url: &str,
    escalation_interval: Duration,
) -> eyre::Result<(Service, TxSitterClient)> {
    let rpc_url = anvil_handle.local_addr();

    let anvil_private_key = hex::encode(DEFAULT_ANVIL_PRIVATE_KEY);

    let config = Config {
        service: TxSitterConfig {
            escalation_interval,
            datadog_enabled: false,
            statsd_enabled: false,
            predefined_networks: vec![PredefinedNetwork {
                chain_id: DEFAULT_ANVIL_CHAIN_ID,
                name: "Anvil".to_string(),
                http_rpc: format!("http://{}", rpc_url),
                ws_rpc: anvil_handle.ws_addr(),
            }],
            predefined_relayers: vec![PredefinedRelayer {
                name: "Anvil".to_string(),
                id: DEFAULT_RELAYER_ID.to_string(),
                key_id: anvil_private_key,
                chain_id: DEFAULT_ANVIL_CHAIN_ID,
            }],
        },
        server: ServerConfig {
            host: SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(127, 0, 0, 1),
                0,
            )),
            username: None,
            password: None,
        },
        database: DatabaseConfig::connection_string(db_connection_url),
        keys: KeysConfig::Local(LocalKeysConfig::default()),
    };

    let service = Service::new(config).await?;

    let client =
        TxSitterClient::new(format!("http://{}", service.local_addr()));

    Ok((service, client))
}

pub async fn setup_middleware(
    rpc_url: impl AsRef<str>,
    private_key: &[u8],
) -> eyre::Result<AppMiddleware> {
    let provider = setup_provider(rpc_url).await?;

    let wallet = LocalWallet::from(SigningKey::from_slice(private_key)?)
        .with_chain_id(provider.get_chainid().await?.as_u64());

    let middleware = SignerMiddleware::new(Arc::new(provider), wallet);

    Ok(middleware)
}

pub async fn setup_provider(
    rpc_url: impl AsRef<str>,
) -> eyre::Result<Provider<Http>> {
    let provider = Provider::<Http>::new(rpc_url.as_ref().parse()?);

    Ok(provider)
}
