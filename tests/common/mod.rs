#![allow(dead_code)] // Needed because this module is imported as module by many test crates

use std::time::Duration;

use alloy::primitives::{address, Address, U256};
use alloy::providers::{Provider, RootProvider};
use postgres_docker_utils::DockerContainerGuard;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

mod anvil_builder;
mod service_builder;

pub use self::anvil_builder::AnvilBuilder;
pub use self::service_builder::ServiceBuilder;

#[allow(unused_imports)]
pub mod prelude {
    pub use std::time::Duration;

    pub use alloy::primitives::utils::{parse_ether, parse_units};
    pub use alloy::primitives::{B256, U256};
    pub use alloy::providers::{Provider, RootProvider};
    pub use futures::stream::FuturesUnordered;
    pub use futures::StreamExt;
    pub use tx_sitter_client::apis::configuration::Configuration;
    pub use tx_sitter_client::models::*;
    pub use url::Url;

    pub use super::*;
}

pub const DEFAULT_ANVIL_ACCOUNT: Address =
    address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266");

pub const DEFAULT_ANVIL_PRIVATE_KEY: &[u8] = &hex_literal::hex!(
    "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
);

pub const SECONDARY_ANVIL_PRIVATE_KEY: &[u8] = &hex_literal::hex!(
    "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
);

pub const ARBITRARY_ADDRESS: Address =
    address!("1Ed53d680B8890DAe2a63f673a85fFDE1FD5C7a2");

pub const DEFAULT_ANVIL_CHAIN_ID: u64 = 31337;
pub const DEFAULT_ANVIL_BLOCK_TIME: u64 = 2;

pub const DEFAULT_RELAYER_ID: &str = "1b908a34-5dc1-4d2d-a146-5eb46e975830";

pub fn setup_tracing() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().pretty().compact())
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .parse_lossy("info,tx_sitter=debug"),
        )
        .init();
}

pub async fn setup_db() -> eyre::Result<(String, DockerContainerGuard)> {
    let db_container = postgres_docker_utils::setup().await?;
    let db_socket_addr = db_container.address();
    let url = format!("postgres://postgres:postgres@{db_socket_addr}/database");

    Ok((url, db_container))
}

pub async fn setup_provider(
    rpc_url: impl AsRef<str>,
) -> eyre::Result<RootProvider> {
    let provider = RootProvider::new_http(rpc_url.as_ref().parse()?);

    Ok(provider)
}

pub async fn await_balance(
    provider: &RootProvider,
    expected_balance: U256,
    address: Address,
) -> eyre::Result<()> {
    for _ in 0..50 {
        let balance = provider.get_balance(address).await?;

        tracing::info!(
            ?address,
            ?balance,
            ?expected_balance,
            "Checking balance"
        );

        if balance >= expected_balance {
            return Ok(());
        } else {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    eyre::bail!("Balance did not reach expected value");
}
