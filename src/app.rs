use alloy::network::{Ethereum, EthereumSigner};
use alloy::primitives::Address;
use alloy::providers::fillers::{
    ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller, SignerFiller,
};
use alloy::providers::{Identity, ProviderBuilder, RootProvider};
use alloy::signers::Signer as AlloySigner;
use alloy::transports::http::{Client, Http as AlloyHttp};
use ethers::middleware::SignerMiddleware;
use ethers::providers::{Http, Provider, Ws};
use ethers::signers::Signer;
use eyre::Context;

use crate::api_key::ApiKey;
use crate::config::{Config, KeysConfig};
use crate::db::data::RpcKind;
use crate::db::Database;
use crate::keys::kms_keys::NewKmsKeys;
use crate::keys::local_keys::NewLocalKeys;
use crate::keys::universal_signer::UniversalSigner;
use crate::keys::{KeysSource, KmsKeys, LocalKeys, NewKeysSource};

pub type AppGenericMiddleware<T> =
    SignerMiddleware<Provider<T>, UniversalSigner>;
pub type AppMiddleware = AppGenericMiddleware<Http>;

pub type UniversalProvider = FillProvider<
    JoinFill<
        JoinFill<
            JoinFill<JoinFill<Identity, GasFiller>, NonceFiller>,
            ChainIdFiller,
        >,
        SignerFiller<EthereumSigner>,
    >,
    RootProvider<AlloyHttp<Client>>,
    AlloyHttp<Client>,
    Ethereum,
>;

pub struct App {
    pub config: Config,

    pub keys_source: Box<dyn KeysSource>,

    pub new_keys_source: Box<dyn NewKeysSource>,

    pub db: Database,
}

impl App {
    pub async fn new(config: Config) -> eyre::Result<Self> {
        let keys_source = init_keys_source(&config).await?;
        let new_keys_source = init_new_keys_source(&config).await?;
        let db = Database::new(&config.database).await?;

        Ok(Self {
            config,
            keys_source,
            new_keys_source,
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

        let ws = Ws::connect(url.as_str()).await?;
        let provider = Provider::new(ws);

        Ok(provider)
    }

    pub async fn universal_provider(
        &self,
        chain_id: u64,
        key_id: String,
    ) -> eyre::Result<(UniversalProvider, Address)> {
        let url = self.db.get_network_rpc(chain_id, RpcKind::Http).await?;

        let wallet = self
            .new_keys_source
            .load_signer(key_id)
            .await
            .context("Missing signer")?;

        let signer = wallet.with_chain_id(Some(chain_id));

        let address = signer.address();

        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .signer(signer.into())
            .on_http(url.parse().unwrap());

        Ok((provider, address))
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

async fn init_new_keys_source(
    config: &Config,
) -> eyre::Result<Box<dyn NewKeysSource>> {
    let keys_source: Box<dyn NewKeysSource> = match &config.keys {
        KeysConfig::Kms(kms_config) => {
            Box::new(NewKmsKeys::new(kms_config).await?)
        }
        KeysConfig::Local(local_config) => {
            Box::new(NewLocalKeys::new(local_config))
        }
    };

    Ok(keys_source)
}
