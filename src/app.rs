use alloy::providers::{ProviderBuilder, RootProvider, WsConnect};
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
}

impl App {
    pub async fn new(config: Config) -> eyre::Result<Self> {
        tracing::info!("Initializing keys source");
        let keys_source = init_keys_source(&config).await?;

        tracing::info!("Initializing db");
        let db = Database::new(&config.database).await?;

        Ok(Self {
            config,
            keys_source,
            db,
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

        Ok(RootProvider::new_http(url))
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
