use std::fmt::Debug;

use alloy::node_bindings::AnvilInstance;
use postgres_docker_utils::DockerContainerGuard;
use tx_sitter::db::Database;
use tx_sitter::service::Service;
use tx_sitter_client::apis::admin_v1_api::{
    CreateRelayerParams, GetRelayerParams, RelayerCreateApiKeyParams,
    UpdateRelayerParams,
};
use tx_sitter_client::apis::configuration::Configuration;
use tx_sitter_client::apis::relayer_v2_api::{
    CreateTransactionV2Error, CreateTransactionV2Params, GetTransactionV2Error,
    GetTransactionV2Params,
};
use tx_sitter_client::apis::Error;
use tx_sitter_client::models::{
    CreateApiKeyResponse, CreateRelayerRequest, CreateRelayerResponse,
    ErrorResponseBody, GetTxResponseV2, RelayerUpdateRequest, SendTxRequestV2,
    SendTxResponse,
};

use super::{
    connect_db, setup_db, AnvilBuilder, ServiceBuilder, DEFAULT_ANVIL_CHAIN_ID,
    DEFAULT_API_KEY, DEFAULT_RELAYER_ID,
};

/// A service on its own database and anvil chain, with an API key for the
/// default anvil relayer
pub struct TestService {
    pub service: Service,
    /// Client for the admin and v1 APIs
    pub client: Configuration,
    pub api_key: String,
    pub anvil: AnvilInstance,
    pub db_url: String,
    pub db: Database,
    _db_container: DockerContainerGuard,
}

impl TestService {
    pub async fn spawn(builder: ServiceBuilder) -> eyre::Result<Self> {
        let (db_url, db_container) = setup_db().await?;
        let anvil = AnvilBuilder::default().spawn().await?;

        let (service, client) = builder.build(&anvil, &db_url).await?;
        let db = connect_db(&db_url).await?;

        Ok(Self {
            service,
            client,
            api_key: DEFAULT_API_KEY.to_string(),
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

    /// Creates another relayer on the anvil chain, with a key of its own and
    /// no funds, and returns its id and an API key for it
    pub async fn create_relayer_with_api_key(
        &self,
    ) -> eyre::Result<(String, String)> {
        let CreateRelayerResponse { relayer_id, .. } =
            tx_sitter_client::apis::admin_v1_api::create_relayer(
                &self.client,
                CreateRelayerParams {
                    create_relayer_request: CreateRelayerRequest::new(
                        "Other relayer".to_string(),
                        DEFAULT_ANVIL_CHAIN_ID as i32,
                    ),
                },
            )
            .await?;

        let api_key = self.create_api_key(&relayer_id).await?;

        Ok((relayer_id, api_key))
    }

    pub async fn create_api_key(
        &self,
        relayer_id: &str,
    ) -> eyre::Result<String> {
        let CreateApiKeyResponse { api_key } =
            tx_sitter_client::apis::admin_v1_api::relayer_create_api_key(
                &self.client,
                RelayerCreateApiKeyParams {
                    relayer_id: relayer_id.to_string(),
                },
            )
            .await?;

        Ok(api_key)
    }

    /// Updates the default relayer
    pub async fn update_relayer(
        &self,
        update: RelayerUpdateRequest,
    ) -> eyre::Result<()> {
        tx_sitter_client::apis::admin_v1_api::update_relayer(
            &self.client,
            UpdateRelayerParams {
                relayer_id: DEFAULT_RELAYER_ID.to_string(),
                relayer_update_request: update,
            },
        )
        .await?;

        Ok(())
    }

    /// Returns the nonce the default relayer gives its next transaction
    pub async fn relayer_nonce(&self) -> eyre::Result<i32> {
        let relayer = tx_sitter_client::apis::admin_v1_api::get_relayer(
            &self.client,
            GetRelayerParams {
                relayer_id: DEFAULT_RELAYER_ID.to_string(),
            },
        )
        .await?;

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
