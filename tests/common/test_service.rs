use std::fmt::Debug;

use alloy::node_bindings::AnvilInstance;
use postgres_docker_utils::DockerContainerGuard;
use tx_sitter::config::RelayerConfig;
use tx_sitter::db::Database;
use tx_sitter::service::Service;
use tx_sitter_client::apis::configuration::Configuration;
use tx_sitter_client::apis::relayer_v2_api::{
    CreateTransactionV2Error, CreateTransactionV2Params, GetTransactionV2Error,
    GetTransactionV2Params,
};
use tx_sitter_client::apis::Error;
use tx_sitter_client::models::{
    ErrorResponseBody, GetTxResponseV2, SendTxRequestV2, SendTxResponse,
};

use super::{
    connect_db, setup_db, sync_relayer_nonces, AnvilBuilder, ServiceBuilder,
    TestRelayer, DEFAULT_ANVIL_ACCOUNT, DEFAULT_ANVIL_CHAIN_ID,
    DEFAULT_API_KEY, DEFAULT_RELAYER_ID,
};

/// A service on its own database and anvil chain, with the default anvil
/// relayer and another relayer on the anvil chain
pub struct TestService {
    pub service: Service,
    /// Client for the v1 API
    pub client: Configuration,
    /// The API key of the default relayer
    pub api_key: String,
    /// A relayer on the anvil chain with a key of its own and no funds
    pub other_relayer: TestRelayer,
    pub anvil: AnvilInstance,
    pub db_url: String,
    pub db: Database,
    _db_container: DockerContainerGuard,
}

impl TestService {
    pub async fn spawn(builder: ServiceBuilder) -> eyre::Result<Self> {
        let (db_url, db_container) = setup_db().await?;
        let anvil = AnvilBuilder::default().spawn().await?;
        let other_relayer = TestRelayer::random(DEFAULT_ANVIL_CHAIN_ID);

        let (service, client) = builder
            .relayer(other_relayer.config.clone())
            .build(&anvil, &db_url)
            .await?;
        sync_relayer_nonces(
            &db_url,
            &anvil,
            &[DEFAULT_ANVIL_ACCOUNT, other_relayer.config.address],
        )
        .await?;
        let db = connect_db(&db_url).await?;

        Ok(Self {
            service,
            client,
            api_key: DEFAULT_API_KEY.to_string(),
            other_relayer,
            anvil,
            db_url,
            db,
            _db_container: db_container,
        })
    }

    pub fn base_url(&self) -> String {
        format!("http://{}", self.service.local_addr())
    }

    /// Returns a client that sends `api_key` as a bearer token
    pub fn v2_client(&self, api_key: &str) -> Configuration {
        Configuration {
            bearer_access_token: Some(api_key.to_string()),
            ..self.client.clone()
        }
    }

    /// Writes whether `relayer` is enabled and its queue limits into the
    /// database, as a service that starts with a config that changed them
    /// does.
    pub async fn update_relayer(
        &self,
        relayer: &RelayerConfig,
    ) -> eyre::Result<()> {
        sqlx::query(
            r#"
            UPDATE relayers
            SET    enabled = $2,
                   max_inflight_txs = $3,
                   max_queued_txs = $4
            WHERE  id = $1
            "#,
        )
        .bind(&relayer.id)
        .bind(relayer.enabled)
        .bind(relayer.max_inflight_txs as i64)
        .bind(relayer.max_queued_txs as i64)
        .execute(&self.db.pool)
        .await?;

        Ok(())
    }

    /// Returns the nonce the default relayer gives its next transaction
    pub async fn relayer_nonce(&self) -> eyre::Result<u64> {
        let relayer = self
            .db
            .get_relayer(DEFAULT_RELAYER_ID)
            .await?
            .ok_or_else(|| eyre::eyre!("Missing default relayer"))?;

        Ok(relayer.nonce)
    }
}

pub async fn send_tx_v2(
    client: &Configuration,
    request: SendTxRequestV2,
) -> Result<SendTxResponse, Error<CreateTransactionV2Error>> {
    tx_sitter_client::apis::relayer_v2_api::create_transaction_v2(
        client,
        CreateTransactionV2Params {
            send_tx_request_v2: request,
        },
    )
    .await
}

pub async fn get_tx_v2(
    client: &Configuration,
    tx_id: &str,
) -> Result<GetTxResponseV2, Error<GetTransactionV2Error>> {
    tx_sitter_client::apis::relayer_v2_api::get_transaction_v2(
        client,
        GetTransactionV2Params {
            tx_id: tx_id.to_string(),
        },
    )
    .await
}

/// Returns the v2 error of a call that failed with `expected`
pub fn expect_v2_error<T: Debug, E: Debug>(
    result: Result<T, Error<E>>,
    expected: reqwest::StatusCode,
) -> eyre::Result<ErrorResponseBody> {
    match result {
        Err(Error::ResponseError(response)) if response.status == expected => {
            Ok(serde_json::from_str(&response.content)?)
        }
        other => eyre::bail!("Expected {expected}, got {other:?}"),
    }
}
