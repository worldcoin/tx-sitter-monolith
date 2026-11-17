use alloy::providers::{ProviderBuilder, RootProvider, WsConnect};
use alloy::rpc::client::RpcClient;
use eyre::Context;
use url::Url;

use crate::api_key::ApiKey;
use crate::config::{Config, KeysConfig};
use crate::db::data::RpcKind;
use crate::db::Database;
use crate::keys::{KeysSource, KmsKeys, LocalKeys, UniversalSigner};

pub struct App {
    pub config: Config,

    pub keys_source: Box<dyn KeysSource>,

    pub db: Database,

    /// Client for HTTP RPC requests, shared by the providers and the RPC
    /// proxy
    pub rpc_client: reqwest::Client,
}

impl App {
    pub async fn new(config: Config) -> eyre::Result<Self> {
        tracing::info!("Initializing keys source");
        let keys_source = init_keys_source(&config).await?;

        tracing::info!("Initializing db");
        let db = Database::new(&config.database).await?;

        let rpc_client = rpc_http_client()?;

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

        let provider = ProviderBuilder::default()
            .connect_ws(WsConnect::new(url))
            .await
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
fn rpc_http_client() -> eyre::Result<reqwest::Client> {
    let root_certs = webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .map(|cert| reqwest::Certificate::from_der(cert))
        .collect::<Result<Vec<_>, _>>()
        .context("Parsing bundled root certificates")?;

    reqwest::Client::builder()
        .tls_certs_only(root_certs)
        .build()
        .context("Building RPC HTTP client")
}
