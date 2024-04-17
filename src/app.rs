use ethers::middleware::SignerMiddleware;
use ethers::providers::{Http, Provider, Ws};
use ethers::signers::Signer;
use eyre::Context;

use crate::api_key::ApiKey;
use crate::config::{Config, KeysConfig};
use crate::db::data::RpcKind;
use crate::db::Database;
use crate::keys::{KeysSource, KmsKeys, LocalKeys, UniversalSigner};

pub type AppGenericMiddleware<T> =
    SignerMiddleware<Provider<T>, UniversalSigner>;
pub type AppMiddleware = AppGenericMiddleware<Http>;

pub struct App {
    pub config: Config,

    pub keys_source: Box<dyn KeysSource>,

    pub db: Database,
}

impl App {
    pub async fn new(config: Config) -> eyre::Result<Self> {
        let keys_source = init_keys_source(&config).await?;
        let db = Database::new(&config.database).await?;

        Ok(Self {
            config,
            keys_source,
            db,
        })
    }

    pub async fn http_provider(
        &self,
        chain_id: u64,
    ) -> eyre::Result<Provider<Http>> {
        let url = self.db.get_network_rpc(chain_id, RpcKind::Http).await?;

        let provider = Provider::<Http>::try_from(url.as_str())?;

        Ok(provider)
    }

    pub async fn ws_provider(
        &self,
        chain_id: u64,
    ) -> eyre::Result<Provider<Ws>> {
        let url = self.db.get_network_rpc(chain_id, RpcKind::Ws).await?;

        let ws = Ws::connect(url.as_str())
            .await
            .context("Connecting to WS provider")?;
        let provider = Provider::new(ws);

        Ok(provider)
    }

    pub async fn signer_middleware(
        &self,
        chain_id: u64,
        key_id: String,
    ) -> eyre::Result<AppMiddleware> {
        let rpc = self.http_provider(chain_id).await?;

        let wallet = self
            .keys_source
            .load_signer(key_id.clone())
            .await
            .context("Missing signer")?;

        let wallet = wallet.with_chain_id(chain_id);

        let middlware = SignerMiddleware::new(rpc, wallet);

        Ok(middlware)
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
