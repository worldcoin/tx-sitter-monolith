#![allow(dead_code)] // Needed because this module is imported as module by many test crates

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use ethers::core::k256::ecdsa::SigningKey;
use ethers::middleware::SignerMiddleware;
use ethers::providers::{Http, Middleware, Provider};
use ethers::signers::LocalWallet;
use ethers::types::{Address, Eip1559TransactionRequest, H160, U256};
use ethers_signers::Signer;
use fake_rpc::DoubleAnvil;
use postgres_docker_utils::DockerContainerGuard;
use service::config::{
    Config, DatabaseConfig, KeysConfig, LocalKeysConfig, RpcConfig,
    ServerConfig, TxSitterConfig,
};
use service::service::Service;
use tokio::task::JoinHandle;

pub type AppMiddleware = SignerMiddleware<Arc<Provider<Http>>, LocalWallet>;

pub const DEFAULT_ANVIL_ACCOUNT: Address = H160(hex_literal::hex!(
    "f39Fd6e51aad88F6F4ce6aB8827279cffFb92266"
));

pub const DEFAULT_ANVIL_PRIVATE_KEY: &[u8] = &hex_literal::hex!(
    "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
);

pub const ARBITRARY_ADDRESS: Address = H160(hex_literal::hex!(
    "1Ed53d680B8890DAe2a63f673a85fFDE1FD5C7a2"
));

pub const DEFAULT_ANVIL_CHAIN_ID: u64 = 31337;

pub struct DoubleAnvilHandle {
    pub double_anvil: Arc<DoubleAnvil>,
    local_addr: SocketAddr,
    server_handle: JoinHandle<eyre::Result<()>>,
}

impl DoubleAnvilHandle {
    pub fn local_addr(&self) -> String {
        self.local_addr.to_string()
    }
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
        DEFAULT_ANVIL_PRIVATE_KEY,
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

    Ok(DoubleAnvilHandle {
        double_anvil,
        local_addr,
        server_handle,
    })
}

pub async fn setup_service(
    rpc_url: &str,
    db_connection_url: &str,
    escalation_interval: Duration,
) -> eyre::Result<Service> {
    println!("rpc_url.to_string() = {}", rpc_url);

    let config = Config {
        service: TxSitterConfig {
            escalation_interval,
        },
        server: ServerConfig {
            host: SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(127, 0, 0, 1),
                0,
            )),
            disable_auth: true,
        },
        rpc: RpcConfig {
            rpcs: vec![format!("http://{}", rpc_url.to_string())],
        },
        database: DatabaseConfig {
            connection_string: db_connection_url.to_string(),
        },
        keys: KeysConfig::Local(LocalKeysConfig {}),
    };

    let service = Service::new(config).await?;

    Ok(service)
}

pub async fn setup_middleware(
    rpc_url: impl AsRef<str>,
    private_key: &[u8],
) -> eyre::Result<AppMiddleware> {
    let provider = Provider::<Http>::new(rpc_url.as_ref().parse()?);

    let wallet = LocalWallet::from(SigningKey::from_slice(private_key)?)
        .with_chain_id(provider.get_chainid().await?.as_u64());

    let middleware = SignerMiddleware::new(Arc::new(provider), wallet);

    Ok(middleware)
}
