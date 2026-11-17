use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::U256;
use alloy::providers::Provider;
use base_api_types::DecimalU256;
use eyre::{Context, ContextCompat};
use poem::http::StatusCode;
use poem::listener::{Acceptor, Listener, TcpListener};
use poem::middleware::Cors;
use poem::web::{Data, LocalAddr};
use poem::{EndpointExt, IntoResponse, Result, Route};
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::{Json, PlainText};
use poem_openapi::{ApiResponse, OpenApi, OpenApiService, Tags};
use security::BasicAuth;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::api_key::ApiKey;
use crate::app::App;
use crate::db::data::RelayerInfo;
use crate::db::CreateResult;
use crate::task_runner::{TaskHealth, TaskRunner};
use crate::types::{
    CreateApiKeyResponse, CreateNetworkRequest, CreateRelayerRequest,
    CreateRelayerResponse, ErrorResponse, GetTxResponse, NetworkResponse,
    RelayerResponse, RelayerUpdateRequest, RpcRequest, SendTxRequest,
    SendTxResponse, TxStatus,
};

mod security;
mod trace_middleware;

const READINESS_DB_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Tags)]
enum OpenAPITags {
    AdminV1,
    RelayerV1,
    Service,
}

struct AdminApi;

#[OpenApi(prefix_path = "/1/admin/")]
impl AdminApi {
    /// Create Relayer
    #[oai(
        path = "/relayer",
        method = "post",
        operation_id = "create_relayer",
        tag = "OpenAPITags::AdminV1"
    )]
    async fn create_relayer(
        &self,
        basic_auth: BasicAuth,
        Data(app): Data<&Arc<App>>,
        Json(req): Json<CreateRelayerRequest>,
    ) -> Result<Json<CreateRelayerResponse>> {
        basic_auth.validate(app).await?;

        let (key_id, signer) = app.keys_source.new_signer(&req.name).await?;

        let address = signer.address();

        let relayer_id = uuid::Uuid::new_v4();
        let relayer_id = relayer_id.to_string();

        app.db
            .create_relayer(
                &relayer_id,
                &req.name,
                req.chain_id,
                &key_id,
                address,
            )
            .await?;

        Ok(Json(CreateRelayerResponse {
            relayer_id,
            address: address.into(),
        }))
    }

    /// Get Relayers
    #[oai(
        path = "/relayers",
        method = "get",
        operation_id = "get_relayers",
        tag = "OpenAPITags::AdminV1"
    )]
    async fn get_relayers(
        &self,
        basic_auth: BasicAuth,
        Data(app): Data<&Arc<App>>,
    ) -> Result<Json<Vec<RelayerResponse>>> {
        basic_auth.validate(app).await?;

        let relayer_info = app.db.get_relayers().await?;

        Ok(Json(relayer_info.into_iter().map(|v| v.into()).collect()))
    }

    /// Get Relayer
    #[oai(
        path = "/relayer/:relayer_id",
        method = "get",
        operation_id = "get_relayer",
        tag = "OpenAPITags::AdminV1"
    )]
    async fn get_relayer(
        &self,
        basic_auth: BasicAuth,
        Data(app): Data<&Arc<App>>,
        Path(relayer_id): Path<String>,
    ) -> Result<Json<RelayerResponse>> {
        basic_auth.validate(app).await?;

        let relayer_info = app.db.get_relayer(&relayer_id).await?;

        match relayer_info {
            Some(relayer_info) => Ok(Json(relayer_info.into())),
            None => Err(poem::error::Error::from_string(
                "Relayer not found".to_string(),
                StatusCode::NOT_FOUND,
            )),
        }
    }

    /// Update Relayer
    #[oai(
        path = "/relayer/:relayer_id",
        method = "post",
        operation_id = "update_relayer",
        tag = "OpenAPITags::AdminV1"
    )]
    async fn update_relayer(
        &self,
        basic_auth: BasicAuth,
        Data(app): Data<&Arc<App>>,
        Path(relayer_id): Path<String>,
        Json(req): Json<RelayerUpdateRequest>,
    ) -> Result<()> {
        basic_auth.validate(app).await?;

        app.db.update_relayer(&relayer_id, &req).await?;

        Ok(())
    }

    /// Reset Relayer transactions
    ///
    /// Purges unsent transactions, useful for unstucking the relayer
    #[oai(
        path = "/relayer/:relayer_id/reset",
        method = "post",
        operation_id = "reset_relayer",
        tag = "OpenAPITags::AdminV1"
    )]
    async fn purge_unsent_txs(
        &self,
        basic_auth: BasicAuth,
        Data(app): Data<&Arc<App>>,
        Path(relayer_id): Path<String>,
    ) -> Result<()> {
        basic_auth.validate(app).await?;

        app.db.purge_unsent_txs(&relayer_id).await?;

        Ok(())
    }

    /// Create Relayer API Key
    #[oai(
        path = "/relayer/:relayer_id/key",
        method = "post",
        operation_id = "relayer_create_api_key",
        tag = "OpenAPITags::AdminV1"
    )]
    async fn create_relayer_api_key(
        &self,
        basic_auth: BasicAuth,
        Data(app): Data<&Arc<App>>,
        Path(relayer_id): Path<String>,
    ) -> Result<Json<CreateApiKeyResponse>> {
        basic_auth.validate(app).await?;

        let api_key = ApiKey::random(&relayer_id);

        app.db
            .upsert_api_key(&relayer_id, api_key.api_key_secret_hash())
            .await?;

        Ok(Json(CreateApiKeyResponse { api_key }))
    }

    /// Create Network
    #[oai(
        path = "/network/:chain_id",
        method = "post",
        operation_id = "create_network",
        tag = "OpenAPITags::AdminV1"
    )]
    async fn create_network(
        &self,
        basic_auth: BasicAuth,
        Data(app): Data<&Arc<App>>,
        Path(chain_id): Path<u64>,
        Json(network): Json<CreateNetworkRequest>,
    ) -> Result<()> {
        basic_auth.validate(app).await?;

        let http_url: Url = network
            .http_rpc
            .parse::<Url>()
            .map_err(poem::error::BadRequest)?;

        let ws_url: Url = network
            .ws_rpc
            .parse::<Url>()
            .map_err(poem::error::BadRequest)?;

        app.db
            .upsert_network(
                chain_id,
                &network.name,
                http_url.as_str(),
                ws_url.as_str(),
            )
            .await?;

        Ok(())
    }

    /// Get Networks
    #[oai(
        path = "/networks",
        method = "get",
        operation_id = "get_networks",
        tag = "OpenAPITags::AdminV1"
    )]
    async fn list_networks(
        &self,
        basic_auth: BasicAuth,
        Data(app): Data<&Arc<App>>,
    ) -> Result<Json<Vec<NetworkResponse>>> {
        basic_auth.validate(app).await?;

        let networks = app.db.get_networks().await?;

        Ok(Json(networks.into_iter().map(|v| v.into()).collect()))
    }
}

struct RelayerApi;

#[OpenApi(prefix_path = "/1/api/")]
impl RelayerApi {
    /// Send Transaction
    #[oai(
        path = "/:api_token/tx",
        method = "post",
        operation_id = "create_transaction",
        tag = "OpenAPITags::RelayerV1"
    )]
    async fn send_tx(
        &self,
        Data(app): Data<&Arc<App>>,
        Path(api_token): Path<ApiKey>,
        Json(req): Json<SendTxRequest>,
    ) -> Result<Json<SendTxResponse>> {
        api_token.validate(app).await?;

        tracing::info!(?req, "Send tx");

        let gas_limit = parse_gas_limit(&req.gas_limit)?;

        let tx_id = if let Some(id) = req.tx_id {
            id
        } else {
            uuid::Uuid::new_v4().to_string()
        };

        let relayer =
            ensure_relayer_accepts_tx(app, api_token.relayer_id(), gas_limit)
                .await?;

        let res = app
            .db
            .create_transaction(
                &tx_id,
                req.to.0,
                req.data.as_ref().map(|d| &d.0[..]).unwrap_or(&[]),
                req.value.0,
                gas_limit,
                req.priority,
                req.blobs,
                api_token.relayer_id(),
                None,
                None,
            )
            .await?;

        match res {
            CreateResult::Created => {}
            // v1 sends store no request hash, so an existing id is always a
            // conflict
            CreateResult::Conflict | CreateResult::Repeat => {
                return Err(SendTxError::TxIdConflict.into())
            }
            CreateResult::QueueFull => {
                return Err(SendTxError::QueueFull.into())
            }
            CreateResult::NonceTaken { nonce } => {
                return Err(
                    SendTxError::nonce_taken(&relayer, &tx_id, nonce).into()
                )
            }
        }

        tracing::info!(
            relayer_id = relayer.id,
            relayer_address = %relayer.address,
            tx_id,
            "Transaction created"
        );

        Ok(Json(SendTxResponse { tx_id }))
    }

    /// Get Transaction
    #[oai(
        path = "/:api_token/tx/:tx_id",
        method = "get",
        operation_id = "get_transaction",
        tag = "OpenAPITags::RelayerV1"
    )]
    async fn get_tx(
        &self,
        Data(app): Data<&Arc<App>>,
        Path(api_token): Path<ApiKey>,
        Path(tx_id): Path<String>,
    ) -> Result<Json<GetTxResponse>> {
        api_token.validate(app).await?;

        let relayer_id = api_token.relayer_id();

        let tx = app
            .db
            .read_relayer_tx(relayer_id, &tx_id)
            .await?
            .ok_or_else(|| {
                poem::error::Error::from_string(
                    "Transaction not found".to_string(),
                    StatusCode::NOT_FOUND,
                )
            })?;

        let get_tx_response = GetTxResponse {
            tx_id: tx.tx_id,
            to: tx.to.into(),
            data: if tx.data.is_empty() {
                None
            } else {
                Some(tx.data.into())
            },
            value: tx.value.into(),
            gas_limit: tx.gas_limit.into(),
            nonce: tx.nonce,
            tx_hash: tx.tx_hash.map(|v| v.into()),
            status: tx.status,
        };

        Ok(Json(get_tx_response))
    }

    /// Get Transactions
    #[oai(
        path = "/:api_token/txs",
        method = "get",
        operation_id = "get_transactions",
        tag = "OpenAPITags::RelayerV1"
    )]
    async fn get_txs(
        &self,
        Data(app): Data<&Arc<App>>,
        Path(api_token): Path<ApiKey>,
        /// Optional tx status to filter by
        Query(status): Query<Option<TxStatus>>,
        /// Fetch unsent txs, overrides the status query
        #[oai(default = "default_false")]
        Query(unsent): Query<bool>,
    ) -> Result<Json<Vec<GetTxResponse>>> {
        api_token.validate(app).await?;

        let txs = if unsent {
            app.db
                .read_relayer_txs(api_token.relayer_id(), Some(None))
                .await?
        } else if let Some(status) = status {
            app.db
                .read_relayer_txs(api_token.relayer_id(), Some(Some(status)))
                .await?
        } else {
            app.db
                .read_relayer_txs(api_token.relayer_id(), None)
                .await?
        };

        let txs = txs
            .into_iter()
            .map(|tx| GetTxResponse {
                tx_id: tx.tx_id,
                to: tx.to.into(),
                data: if tx.data.is_empty() {
                    None
                } else {
                    Some(tx.data.into())
                },
                value: tx.value.into(),
                gas_limit: tx.gas_limit.into(),
                nonce: tx.nonce,
                tx_hash: tx.tx_hash.map(|v| v.into()),
                status: tx.status,
            })
            .collect();

        Ok(Json(txs))
    }

    /// Relayer RPC
    #[oai(
        path = "/:api_token/rpc",
        method = "post",
        operation_id = "call_rpc",
        tag = "OpenAPITags::RelayerV1"
    )]
    async fn relayer_rpc(
        &self,
        Data(app): Data<&Arc<App>>,
        Path(api_token): Path<ApiKey>,
        Json(req): Json<RpcRequest>,
    ) -> Result<Json<Value>> {
        api_token.validate(app).await?;

        let relayer_info = app
            .db
            .get_relayer(api_token.relayer_id())
            .await?
            .context("Missing relayer")?;

        // TODO: Cache?
        let url = app.http_rpc_url(relayer_info.chain_id).await?;

        // RPC URLs usually embed a provider API key, so neither the error
        // returned to the caller nor the log may contain the URL.
        let upstream_error = |stage: &str, err: reqwest::Error| {
            tracing::warn!(
                relayer_id = relayer_info.id,
                chain_id = relayer_info.chain_id,
                error = %err.without_url(),
                "RPC proxy {stage} failed"
            );

            poem::Error::from_string(
                "RPC request to the upstream node failed",
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        };

        let response = app
            .rpc_client
            .post(url)
            .json(&req)
            .send()
            .await
            .map_err(|err| upstream_error("request", err))?;

        let response: Value = response
            .json()
            .await
            .map_err(|err| upstream_error("response parsing", err))?;

        Ok(Json(response))
    }
}

fn default_false() -> bool {
    false
}

fn parse_gas_limit(gas_limit: &DecimalU256) -> Result<U256> {
    let gas_limit: u64 = gas_limit.0.try_into().map_err(|_| {
        poem::error::Error::from_string(
            format!("Gas limit must be at most {}", u64::MAX),
            StatusCode::BAD_REQUEST,
        )
    })?;

    Ok(U256::from(gas_limit))
}

/// Why a send didn't store its transaction.
#[derive(Debug)]
enum SendTxError {
    RelayerDisabled,
    QueueFull,
    /// The relayer's balance doesn't cover the gas of its queued
    /// transactions and the new one at the latest gas price
    InsufficientFunds,
    /// A different request already took the transaction id
    TxIdConflict,
    /// Another transaction of the relayer holds its next nonce, which a
    /// relayer reset repairs
    NonceTaken,
    /// The RPC didn't return the relayer's balance
    BalanceUnavailable(eyre::Report),
    Internal(eyre::Report),
}

impl SendTxError {
    /// Reports that `create_transaction` found the next nonce of `relayer`
    /// taken.
    fn nonce_taken(relayer: &RelayerInfo, tx_id: &str, nonce: u64) -> Self {
        tracing::error!(
            relayer_id = relayer.id,
            relayer_address = %relayer.address,
            chain_id = relayer.chain_id,
            tx_id,
            nonce,
            "Relayer's next nonce is taken by another of its transactions, reset the relayer"
        );

        Self::NonceTaken
    }
}

impl From<eyre::Report> for SendTxError {
    fn from(err: eyre::Report) -> Self {
        Self::Internal(err)
    }
}

impl From<SendTxError> for poem::Error {
    fn from(err: SendTxError) -> Self {
        match err {
            SendTxError::RelayerDisabled => poem::Error::from_string(
                "Relayer is disabled".to_string(),
                StatusCode::FORBIDDEN,
            ),
            SendTxError::QueueFull => poem::Error::from_string(
                "Relayer queue is full".to_string(),
                StatusCode::TOO_MANY_REQUESTS,
            ),
            SendTxError::InsufficientFunds => poem::Error::from_string(
                "Relayer funds are insufficient for transaction to be mined."
                    .to_string(),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
            SendTxError::TxIdConflict => poem::Error::from_response(
                ErrorResponse::new(
                    StatusCode::CONFLICT,
                    "transaction_already_exists",
                    "Transaction with same id already exists.",
                )
                .into_response(),
            ),
            SendTxError::NonceTaken => poem::Error::from_response(
                ErrorResponse::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "relayer_nonce_taken",
                    "The relayer's next nonce is taken by another of its transactions. Resetting the relayer repairs it.",
                )
                .into_response(),
            ),
            SendTxError::BalanceUnavailable(err) | SendTxError::Internal(err) => {
                err.into()
            }
        }
    }
}

/// Checks that relayer `relayer_id` can take a new transaction with
/// `gas_limit`, and returns the relayer.
///
/// Its queue is checked here only to refuse early, before the balance check
/// calls the RPC: `create_transaction` admits the transaction.
async fn ensure_relayer_accepts_tx(
    app: &App,
    relayer_id: &str,
    gas_limit: U256,
) -> Result<RelayerInfo, SendTxError> {
    let relayer = app
        .db
        .get_relayer(relayer_id)
        .await?
        .context("Missing relayer")?;

    if !relayer.enabled {
        return Err(SendTxError::RelayerDisabled);
    }

    let relayer_queued_tx_count =
        app.db.get_relayer_pending_txs(relayer_id).await?;

    if relayer_queued_tx_count >= relayer.max_queued_txs as usize {
        return Err(SendTxError::QueueFull);
    }

    let relayer_queued_tx_gas_limit_sum = app
        .db
        .get_relayer_pending_txs_gas_limit_sum(relayer_id)
        .await?;

    let block_fees = app
        .db
        .get_latest_block_fees_by_chain_id(relayer.chain_id)
        .await?;
    if let Some(block_fees) = block_fees {
        let total_gas_limit =
            relayer_queued_tx_gas_limit_sum.saturating_add(gas_limit);
        let estimated_transactions_cost =
            block_fees.gas_price.saturating_mul(total_gas_limit);

        // TODO: Cache?
        let http_provider = app.http_provider(relayer.chain_id).await?;

        let balance = http_provider
            .get_balance(relayer.address.0)
            .await
            .map_err(|err| {
                SendTxError::BalanceUnavailable(eyre::eyre!(
                    "Error checking balance: {}",
                    err
                ))
            })?;

        if balance < estimated_transactions_cost {
            tracing::warn!(
                relayer_id = relayer.id,
                relayer_address = %relayer.address,
                balance = %balance,
                estimated_transactions_cost = %estimated_transactions_cost,
                "Relayer funds are insufficient for transaction to be mined"
            );

            return Err(SendTxError::InsufficientFunds);
        }
    }

    Ok(relayer)
}

struct ServiceApi;

#[derive(ApiResponse)]
enum HealthResponse {
    #[oai(status = 200)]
    Healthy,
    /// The reason the check failed
    #[oai(status = 503)]
    Unhealthy(PlainText<String>),
}

#[OpenApi]
impl ServiceApi {
    /// Health
    ///
    /// Alias of `/health/live`
    #[oai(
        path = "/health",
        method = "get",
        operation_id = "health",
        tag = "OpenAPITags::Service"
    )]
    async fn health(
        &self,
        Data(task_runner): Data<&Arc<TaskRunner<App>>>,
    ) -> HealthResponse {
        check_liveness(task_runner)
    }

    /// Liveness
    ///
    /// Fails once a background task stops or misses its progress deadline
    #[oai(
        path = "/health/live",
        method = "get",
        operation_id = "health_live",
        tag = "OpenAPITags::Service"
    )]
    async fn health_live(
        &self,
        Data(task_runner): Data<&Arc<TaskRunner<App>>>,
    ) -> HealthResponse {
        check_liveness(task_runner)
    }

    /// Readiness
    ///
    /// Fails while the database is unreachable
    #[oai(
        path = "/health/ready",
        method = "get",
        operation_id = "health_ready",
        tag = "OpenAPITags::Service"
    )]
    async fn health_ready(&self, Data(app): Data<&Arc<App>>) -> HealthResponse {
        match app.db.ping(READINESS_DB_TIMEOUT).await {
            Ok(()) => HealthResponse::Healthy,
            Err(err) => {
                tracing::warn!(error = ?err, "Readiness check failed");

                HealthResponse::Unhealthy(PlainText(
                    "Database is unreachable".to_string(),
                ))
            }
        }
    }
}

fn check_liveness(task_runner: &TaskRunner<App>) -> HealthResponse {
    let unhealthy_tasks: Vec<String> = task_runner
        .task_health()
        .into_iter()
        .filter(|(_, health)| *health != TaskHealth::Progressing)
        .map(|(label, health)| format!("{label} ({health:?})"))
        .collect();

    if unhealthy_tasks.is_empty() {
        HealthResponse::Healthy
    } else {
        HealthResponse::Unhealthy(PlainText(format!(
            "Unhealthy tasks: {}",
            unhealthy_tasks.join(", ")
        )))
    }
}

pub struct ServerHandle {
    pub local_addrs: Vec<LocalAddr>,
    pub server_handle: tokio::task::JoinHandle<eyre::Result<()>>,
}

impl ServerHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addrs
            .iter()
            .filter_map(|addr| addr.as_socket_addr())
            .next()
            .cloned()
            .expect("Not bound to any address")
    }
}

pub async fn generate_spec_yaml(server_address: Option<&String>) -> String {
    let api_service = create_api_service(server_address);

    api_service.spec_yaml()
}

/// Serves the HTTP endpoints of the configured role until `shutdown` is
/// cancelled.
///
/// After that the server stops accepting connections and gives in-flight
/// requests `server.shutdown_timeout` to finish, logging a warning if they
/// don't. If the server fails or panics, it cancels `shutdown` so the rest of
/// the service stops too.
pub async fn spawn_server(
    app: Arc<App>,
    task_runner: Arc<TaskRunner<App>>,
    shutdown: CancellationToken,
) -> eyre::Result<ServerHandle> {
    let routes = if app.config.service.role.serves_api() {
        api_routes(app.config.server.server_address.as_ref())
    } else {
        health_routes()
    };

    let router = routes
        .with(trace_middleware::TraceMiddleware)
        .with(Cors::new())
        .data(app.clone())
        .data(task_runner);

    let listener = TcpListener::bind(app.config.server.host);
    let acceptor = listener.into_acceptor().await?;

    let local_addrs = acceptor.local_addr();

    let server = poem::Server::new_with_acceptor(acceptor);
    let shutdown_timeout = app.config.server.shutdown_timeout;

    let server_handle = tokio::spawn(async move {
        let _cancel_on_exit = shutdown.clone().drop_guard();

        let serve = server.run_with_graceful_shutdown(
            router,
            shutdown.clone().cancelled_owned(),
            None,
        );
        let drain_deadline = async {
            shutdown.cancelled().await;
            tokio::time::sleep(shutdown_timeout).await;
        };

        tokio::select! {
            result = serve => result.context("Running the HTTP server"),
            () = drain_deadline => {
                tracing::warn!(
                    ?shutdown_timeout,
                    "In-flight requests did not finish within the shutdown timeout, dropping them"
                );

                Ok(())
            }
        }
    });

    Ok(ServerHandle {
        local_addrs,
        server_handle,
    })
}

fn api_routes(server_address: Option<&String>) -> Route {
    let api_service = create_api_service(server_address);

    Route::new()
        .nest("/rapidoc", api_service.rapidoc())
        .nest("/swagger", api_service.swagger_ui())
        .nest("/redoc", api_service.redoc())
        .nest("/schema.json", api_service.spec_endpoint())
        .nest("/schema.yml", api_service.spec_endpoint_yaml())
        .nest("/", api_service)
}

fn health_routes() -> Route {
    let health_service =
        OpenApiService::new(ServiceApi, "Tx Sitter", version::version!());

    Route::new().nest("/", health_service)
}

fn create_api_service(
    server_address: Option<&String>,
) -> OpenApiService<(AdminApi, RelayerApi, ServiceApi), ()> {
    let mut api_service = OpenApiService::new(
        (AdminApi, RelayerApi, ServiceApi),
        "Tx Sitter",
        version::version!(),
    )
    .description(include_str!("./server/description.md"));

    if let Some(server_address) = server_address {
        api_service = api_service.server(server_address.clone());
    }

    api_service
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The address `api_spec_generator` uses by default.
    const SPEC_SERVER_ADDRESS: &str = "http://localhost:8000";

    #[tokio::test]
    async fn openapi_spec_matches_snapshot() {
        let spec =
            generate_spec_yaml(Some(&SPEC_SERVER_ADDRESS.to_string())).await;

        assert!(
            format!("{spec}\n") == include_str!("./server/openapi.yaml"),
            "The OpenAPI spec changed. tx-sitter-client is generated from it, \
             so check the change is intended, then run \
             `cargo run --bin api_spec_generator > src/server/openapi.yaml` \
             and regenerate the client"
        );
    }
}
