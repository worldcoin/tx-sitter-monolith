use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use ethers::middleware::SignerMiddleware;
use ethers::providers::{Http, Middleware, Provider};
use ethers::types::{BlockNumber, U256};
use ethers_signers::Signer;
use eyre::{Context, ContextCompat};

use crate::config::{Config, KeysConfig};
use crate::db::{BlockTxStatus, Database};
use crate::keys::{KeysSource, KmsKeys, LocalKeys, UniversalSigner};
use crate::tasks::index::fetch_block_with_fee_estimates;

pub type AppMiddleware = SignerMiddleware<Arc<Provider<Http>>, UniversalSigner>;

pub struct App {
    pub config: Config,

    pub rpcs: HashMap<U256, Arc<Provider<Http>>>,

    pub keys_source: Box<dyn KeysSource>,

    pub db: Database,
}

impl App {
    pub async fn new(config: Config) -> eyre::Result<Self> {
        let rpcs = init_rpcs(&config).await?;
        let keys_source = init_keys_source(&config).await?;
        let db = Database::new(&config.database).await?;

        seed_initial_blocks(&rpcs, &db).await?;

        Ok(Self {
            config,
            rpcs,
            keys_source,
            db,
        })
    }

    pub async fn fetch_signer_middleware(
        &self,
        chain_id: impl Into<U256>,
        key_id: String,
    ) -> eyre::Result<AppMiddleware> {
        let chain_id: U256 = chain_id.into();

        let rpc = self
            .rpcs
            .get(&chain_id)
            .context("Missing RPC for chain id")?
            .clone();

        let wallet = self
            .keys_source
            .load_signer(key_id.clone())
            .await
            .context("Missing signer")?;

        let wallet = wallet.with_chain_id(chain_id.as_u64());

        let middlware = SignerMiddleware::new(rpc, wallet);

        Ok(middlware)
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

async fn init_rpcs(
    config: &Config,
) -> eyre::Result<HashMap<U256, Arc<Provider<Http>>>> {
    let mut providers = HashMap::new();

    for rpc_url in &config.rpc.rpcs {
        let provider = Provider::<Http>::try_from(rpc_url.as_str())?;
        let chain_id = provider.get_chainid().await?;

        providers.insert(chain_id, Arc::new(provider));
    }

    Ok(providers)
}

async fn seed_initial_blocks(
    rpcs: &HashMap<U256, Arc<Provider<Http>>>,
    db: &Database,
) -> eyre::Result<()> {
    for (chain_id, rpc) in rpcs {
        tracing::info!("Seeding block for chain id {chain_id}");

        if !db.has_blocks_for_chain(chain_id.as_u64()).await? {
            let (block, fee_estimates) =
                fetch_block_with_fee_estimates(rpc, BlockNumber::Latest)
                    .await?
                    .context("Missing latest block")?;

            let block_timestamp_seconds = block.timestamp.as_u64();
            let block_timestamp = DateTime::<Utc>::from_timestamp(
                block_timestamp_seconds as i64,
                0,
            )
            .context("Invalid timestamp")?;

            db.save_block(
                block.number.context("Missing block number")?.as_u64(),
                chain_id.as_u64(),
                block_timestamp,
                &block.transactions,
                Some(&fee_estimates),
                BlockTxStatus::Mined,
            )
            .await?;
        }
    }

    Ok(())
}
