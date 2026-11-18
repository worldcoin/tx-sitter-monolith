#![allow(dead_code)] // Needed because this module is imported as module by many test crates

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use alloy::node_bindings::AnvilInstance;
use alloy::primitives::{address, Address, B256, U256};
use alloy::providers::{Provider, RootProvider};
use alloy::signers::k256::ecdsa::SigningKey;
use alloy::signers::local::PrivateKeySigner;
use postgres_docker_utils::DockerContainerGuard;
use tokio::net::TcpListener;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;
use tx_sitter::api_key::ApiKey;
use tx_sitter::config::{
    DatabaseConfig, NetworkConfig, RelayerConfig, RelayerKey,
};
use tx_sitter::db::data::ChainNonces;
use tx_sitter::db::Database;
use tx_sitter::service::Service;
use tx_sitter::types::secret_string::SecretString;
use tx_sitter::types::secret_url::SecretUrl;
use tx_sitter_client::apis::configuration::Configuration;
use tx_sitter_client::apis::relayer_v1_api::{
    CreateTransactionParams, GetTransactionParams,
};
use tx_sitter_client::models::{SendTxRequest, TxStatus};

mod anvil_builder;
mod service_builder;
mod test_service;

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
    pub use reqwest::StatusCode;
    pub use tokio_util::sync::CancellationToken;
    pub use tx_sitter::config::{NetworkConfig, RelayerConfig, ServiceRole};
    pub use tx_sitter_client::apis::configuration::Configuration;
    pub use tx_sitter_client::models::*;
    pub use url::Url;

    pub use super::test_service::*;
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

/// The API key of the default relayer
pub const DEFAULT_API_KEY: &str =
    "G5CKNF3BTS2hRl60bpdYMNPqXvXsP-QZd2lrtmgctsk=";

/// The anvil chain, indexed through `anvil`'s RPCs.
pub fn anvil_network(anvil: &AnvilInstance) -> NetworkConfig {
    NetworkConfig {
        chain_id: DEFAULT_ANVIL_CHAIN_ID,
        name: "Anvil".to_string(),
        http_rpcs: vec![rpc_url(&anvil.endpoint())],
        ws_rpc: rpc_url(&anvil.ws_endpoint()),
        op_stack: None,
    }
}

pub fn rpc_url(url: &str) -> SecretUrl {
    SecretUrl::new(url.parse().expect("Invalid RPC URL"))
}

/// The relayer of the default anvil account, which accepts
/// [`DEFAULT_API_KEY`].
pub fn default_relayer() -> RelayerConfig {
    RelayerConfig {
        id: DEFAULT_RELAYER_ID.to_string(),
        name: "Anvil".to_string(),
        api_key_hashes: vec![api_key_hash(DEFAULT_API_KEY)],
        ..relayer_with_key(DEFAULT_ANVIL_CHAIN_ID, DEFAULT_ANVIL_PRIVATE_KEY)
    }
}

/// A relayer with a random id, local key and API key.
pub struct TestRelayer {
    pub config: RelayerConfig,
    pub api_key: String,
}

impl TestRelayer {
    pub fn random(chain_id: u64) -> Self {
        let private_key = SigningKey::random(&mut rand::rngs::OsRng).to_bytes();

        Self::with_key(chain_id, &private_key)
    }

    /// A relayer with a random id and API key that signs with `private_key`.
    pub fn with_key(chain_id: u64, private_key: &[u8]) -> Self {
        let config = relayer_with_key(chain_id, private_key);
        let api_key = ApiKey::random(&config.id)
            .reveal()
            .expect("Relayer id is a UUID");

        Self {
            config: RelayerConfig {
                api_key_hashes: vec![api_key_hash(&api_key)],
                ..config
            },
            api_key,
        }
    }
}

/// A relayer with a random id that signs with `private_key` and accepts no
/// API keys.
fn relayer_with_key(chain_id: u64, private_key: &[u8]) -> RelayerConfig {
    let signer =
        PrivateKeySigner::from_slice(private_key).expect("Invalid private key");

    RelayerConfig {
        id: uuid::Uuid::new_v4().to_string(),
        name: "Test relayer".to_string(),
        chain_id,
        key: RelayerKey::Local(SecretString::new(hex::encode(private_key))),
        address: signer.address(),
        max_inflight_txs: 5,
        max_queued_txs: 20,
        gas_price_limits: vec![],
        enabled: true,
        api_key_hashes: vec![],
    }
}

pub fn api_key_hash(api_key: &str) -> B256 {
    let api_key: ApiKey = api_key.parse().expect("Invalid API key");

    B256::from(api_key.api_key_secret_hash())
}

/// Installs the log subscriber, unless an earlier test in the same binary
/// already did.
pub fn setup_tracing() {
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().pretty().compact())
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .parse_lossy("info,tx_sitter=debug"),
        )
        .try_init();
}

pub async fn setup_db() -> eyre::Result<(String, DockerContainerGuard)> {
    let db_container = postgres_docker_utils::setup().await?;
    let db_socket_addr = db_container.address();
    let url = format!("postgres://postgres:postgres@{db_socket_addr}/database");

    Ok((url, db_container))
}

/// A TCP server that accepts connections and never answers.
pub struct UnresponsiveRpc {
    pub addr: SocketAddr,
    accepted_connections: Arc<AtomicUsize>,
}

impl UnresponsiveRpc {
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

    /// A network `chain_id` whose HTTP and WS RPCs are this server.
    pub fn network(&self, chain_id: u64) -> NetworkConfig {
        NetworkConfig {
            chain_id,
            name: "Unresponsive".to_string(),
            http_rpcs: vec![rpc_url(&format!("http://{}", self.addr))],
            ws_rpc: rpc_url(&format!("ws://{}", self.addr)),
            op_stack: None,
        }
    }
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

/// Sends `value` wei to [`ARBITRARY_ADDRESS`] through tx-sitter and returns
/// the transaction id.
pub async fn send_transfer(
    client: &Configuration,
    api_key: &str,
    value: U256,
) -> eyre::Result<String> {
    let response = tx_sitter_client::apis::relayer_v1_api::create_transaction(
        client,
        CreateTransactionParams {
            api_token: api_key.to_owned(),
            send_tx_request: SendTxRequest {
                to: ARBITRARY_ADDRESS.into(),
                value: value.into(),
                gas_limit: U256::from(21_000).into(),
                ..Default::default()
            },
        },
    )
    .await?;

    Ok(response.tx_id)
}

/// Waits until tx-sitter reports the transaction as mined and returns the
/// hash it stored when broadcasting.
pub async fn await_mined_tx_hash(
    client: &Configuration,
    api_key: &str,
    tx_id: &str,
) -> eyre::Result<base_api_types::H256> {
    for _ in 0..30 {
        let tx = tx_sitter_client::apis::relayer_v1_api::get_transaction(
            client,
            GetTransactionParams {
                api_token: api_key.to_owned(),
                tx_id: tx_id.to_owned(),
            },
        )
        .await?;

        if let (Some(TxStatus::Mined | TxStatus::Finalized), Some(tx_hash)) =
            (tx.status, tx.tx_hash)
        {
            return Ok(tx_hash);
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    eyre::bail!("Transaction {tx_id} was not mined in time");
}

/// Waits until `service` has fee estimates for `chain_id`.
pub async fn await_estimates(
    service: &Service,
    chain_id: u64,
    timeout: Duration,
) -> eyre::Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;

    while tokio::time::Instant::now() < deadline {
        if service.are_estimates_ready_for_chain(chain_id).await {
            return Ok(());
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    eyre::bail!("Estimates were not ready!");
}

/// Connects to the database at `db_url`, so a test can read and write it
/// without going through a service.
pub async fn connect_db(db_url: &str) -> eyre::Result<Database> {
    Database::new(&DatabaseConfig::connection_string(db_url)).await
}

/// Syncs the nonces of the anvil relayers with `addresses` from `anvil`, as
/// a worker does at startup, for tests without a worker.
pub async fn sync_relayer_nonces(
    db_url: &str,
    anvil: &AnvilInstance,
    addresses: &[Address],
) -> eyre::Result<()> {
    let db = connect_db(db_url).await?;
    let provider = setup_provider(anvil.endpoint()).await?;

    for &address in addresses {
        let nonces = ChainNonces {
            latest: provider.get_transaction_count(address).await?,
            pending: Some(
                provider.get_transaction_count(address).pending().await?,
            ),
        };
        db.update_relayer_nonce(DEFAULT_ANVIL_CHAIN_ID, address, nonces)
            .await?;
    }

    Ok(())
}

/// Sends `GET path` to the service at `addr` and returns the response status
/// and body.
pub async fn http_get(
    addr: SocketAddr,
    path: &str,
) -> eyre::Result<(reqwest::StatusCode, String)> {
    let response = reqwest::get(format!("http://{addr}{path}")).await?;
    let status = response.status();
    let body = response.text().await?;

    Ok((status, body))
}

/// Polls `GET path` on the service at `addr` until it returns `expected`, and
/// returns the response body.
pub async fn await_status(
    addr: SocketAddr,
    path: &str,
    expected: reqwest::StatusCode,
    timeout: Duration,
) -> eyre::Result<String> {
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let (status, body) = http_get(addr, path).await?;
        if status == expected {
            return Ok(body);
        }

        if tokio::time::Instant::now() >= deadline {
            eyre::bail!("{path} returned {status} instead of {expected} within {timeout:?}: {body}");
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
