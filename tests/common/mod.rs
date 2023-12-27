#![allow(dead_code)] // Needed because this module is imported as module by many test crates

use std::sync::Arc;

use ethers::core::k256::ecdsa::SigningKey;
use ethers::middleware::SignerMiddleware;
use ethers::providers::{Http, Middleware, Provider};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::{Address, H160};
use postgres_docker_utils::DockerContainerGuard;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

pub type AppMiddleware = SignerMiddleware<Arc<Provider<Http>>, LocalWallet>;

mod anvil_builder;
mod service_builder;

pub use self::anvil_builder::AnvilBuilder;
pub use self::service_builder::ServiceBuilder;

#[allow(unused_imports)]
pub mod prelude {
    pub use std::time::Duration;

    pub use ethers::prelude::{Http, Provider};
    pub use ethers::providers::Middleware;
    pub use ethers::types::{Eip1559TransactionRequest, H256, U256};
    pub use ethers::utils::parse_units;
    pub use futures::stream::FuturesUnordered;
    pub use futures::StreamExt;
    pub use tx_sitter::api_key::ApiKey;
    pub use tx_sitter::client::TxSitterClient;
    pub use tx_sitter::server::routes::relayer::{
        CreateApiKeyResponse, CreateRelayerRequest, CreateRelayerResponse,
    };
    pub use tx_sitter::server::routes::transaction::SendTxRequest;
    pub use url::Url;

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
pub const DEFAULT_ANVIL_BLOCK_TIME: u64 = 2;

pub const DEFAULT_RELAYER_ID: &str = "1b908a34-5dc1-4d2d-a146-5eb46e975830";

pub fn setup_tracing() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().pretty().compact())
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                // Logging from fake_rpc can get very messy so we set it to warn only
                .parse_lossy("info,tx_sitter=debug,fake_rpc=warn"),
        )
        .init();
}

pub async fn setup_db() -> eyre::Result<(String, DockerContainerGuard)> {
    let db_container = postgres_docker_utils::setup().await?;
    let db_socket_addr = db_container.address();
    let url = format!("postgres://postgres:postgres@{db_socket_addr}/database");

    Ok((url, db_container))
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
