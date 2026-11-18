use std::collections::HashMap;
use std::time::Duration;

use alloy::providers::{ProviderBuilder, RootProvider, WsConnect};
use alloy::rpc::client::RpcClient;
use alloy::transports::utils::guess_local_url;
use eyre::{Context, ContextCompat};

use crate::api_key::ApiKey;
use crate::broadcast_utils::l1_fees::L1FeeGate;
use crate::config::{Config, KeysConfig, NetworkConfig};
use crate::db::data::ApiKeyStatus;
use crate::db::Database;
use crate::failover_transport::FailoverTransport;
use crate::keys::{KeysSource, KmsKeys, LocalKeys, UniversalSigner};

const RPC_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct App {
    pub config: Config,

    pub keys_source: Box<dyn KeysSource>,

    pub db: Database,

    /// The HTTP RPCs of every configured network by chain id, with requests
    /// bounded by `service.rpc_timeout`
    http_rpcs: HashMap<u64, FailoverTransport>,

    /// The L1 fee gates of the OP-stack networks by chain id, which start
    /// from the L1 fees the database holds
    l1_fee_gates: HashMap<u64, L1FeeGate>,
}

impl App {
    pub async fn new(config: Config) -> eyre::Result<Self> {
        tracing::info!("Initializing keys source");
        let keys_source = init_keys_source(&config).await?;

        tracing::info!("Initializing db");
        let db = Database::new(&config.database).await?;

        let rpc_client = rpc_http_client(config.service.rpc_timeout)?;
        let http_rpcs = config
            .networks
            .iter()
            .map(|network| {
                let transport = FailoverTransport::new(
                    network.chain_id,
                    &network.http_rpcs,
                    &rpc_client,
                );

                (network.chain_id, transport)
            })
            .collect();

        let mut l1_fee_gates = HashMap::new();
        for network in &config.networks {
            let Some(caps) = network.op_stack.clone() else {
                continue;
            };
            let chain_id = network.chain_id;
            let fees = db.get_l1_fees(chain_id).await?;

            l1_fee_gates.insert(chain_id, L1FeeGate::new(chain_id, caps, fees));
        }

        Ok(Self {
            config,
            keys_source,
            db,
            http_rpcs,
            l1_fee_gates,
        })
    }

    /// Returns a provider that sends requests to the HTTP RPCs of `chain_id`,
    /// failing over between them, see [`FailoverTransport`].
    pub async fn http_provider(
        &self,
        chain_id: u64,
    ) -> eyre::Result<RootProvider> {
        let transport = self.http_rpcs.get(&chain_id).with_context(|| {
            format!("Network {chain_id} is not in the config")
        })?;
        let is_local = self
            .network(chain_id)?
            .http_rpcs
            .iter()
            .all(|url| guess_local_url(url.expose()));

        let client = RpcClient::new(transport.clone(), is_local);

        Ok(RootProvider::new(client))
    }

    /// Connects to the WS RPC of `chain_id`.
    ///
    /// Connecting isn't bounded, so callers bound it, for example by
    /// `service.rpc_timeout`.
    pub async fn ws_provider(
        &self,
        chain_id: u64,
    ) -> eyre::Result<RootProvider> {
        let url = self.network(chain_id)?.ws_rpc.expose().to_string();

        ProviderBuilder::default()
            .connect_ws(WsConnect::new(url))
            .await
            .context("Connecting to WS provider")
    }

    /// Returns the L1 fee gate of `chain_id`, or `None` unless the network
    /// configures `op_stack`.
    pub fn l1_fee_gate(&self, chain_id: u64) -> Option<&L1FeeGate> {
        self.l1_fee_gates.get(&chain_id)
    }

    fn network(&self, chain_id: u64) -> eyre::Result<&NetworkConfig> {
        self.config
            .networks
            .iter()
            .find(|network| network.chain_id == chain_id)
            .with_context(|| format!("Network {chain_id} is not in the config"))
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

    pub async fn api_key_status(
        &self,
        api_token: &ApiKey,
    ) -> eyre::Result<ApiKeyStatus> {
        self.db
            .get_api_key_status(
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
    use std::time::Instant;

    use alloy::providers::Provider;
    use alloy::transports::{RpcError, TransportErrorKind};
    use eyre::ContextCompat;
    use tokio::time::timeout;

    use super::*;
    use crate::test_utils::{
        setup_app, UnresponsiveServer, CHAIN_ID, RPC_TIMEOUT,
    };

    #[tokio::test]
    async fn http_rpc_call_fails_within_rpc_timeout() -> eyre::Result<()> {
        let rpc = UnresponsiveServer::spawn().await?;
        let (app, _db_container) = setup_app(
            &format!("http://{}", rpc.addr),
            &format!("ws://{}", rpc.addr),
        )
        .await?;
        let provider = app.http_provider(CHAIN_ID).await?;

        let started = Instant::now();
        let result = timeout(RPC_TIMEOUT * 3, provider.get_block_number())
            .await
            .context("RPC call outlived rpc_timeout")?;
        let elapsed = started.elapsed();

        let Err(RpcError::Transport(TransportErrorKind::Custom(err))) = result
        else {
            eyre::bail!("Expected a transport error, got {result:?}");
        };
        let err = err
            .downcast_ref::<reqwest::Error>()
            .context("Expected a reqwest error")?;
        assert!(err.is_timeout(), "Unexpected error: {err:?}");
        assert!(elapsed >= RPC_TIMEOUT, "Took {elapsed:?}");
        assert!(elapsed <= RPC_TIMEOUT * 2, "Took {elapsed:?}");

        Ok(())
    }
}
