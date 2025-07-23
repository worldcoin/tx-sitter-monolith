use std::net::SocketAddr;
use std::sync::Arc;

use ethers::middleware::Middleware;
use ethers::providers::{Http, Provider};
use ethers::signers::Signer;
use eyre::ContextCompat;
use poem::http::StatusCode;
use poem::listener::{Acceptor, Listener, TcpListener};
use poem::middleware::Cors;
use poem::web::{Data, LocalAddr};
use poem::{EndpointExt, IntoResponse, Result, Route};
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::Json;
use poem_openapi::{ApiResponse, OpenApi, OpenApiService, Tags};
use security::BasicAuth;
use serde_json::Value;
use url::Url;

use crate::api_key::ApiKey;
use crate::app::App;
use crate::db::CreateResult;
use crate::service::Service;
use crate::task_runner::TaskRunner;
use crate::types::{CreateApiKeyResponse, CreateNetworkRequest, CreateRelayerRequest, CreateRelayerResponse, ErrorResponse, GetTxResponse, NetworkResponse, RelayerResponse, RelayerUpdateRequest, RpcRequest, SendTxRequest, SendTxResponse, TxStatus};

mod security;
mod trace_middleware;

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

        let task_runner = TaskRunner::new(app.clone());
        Service::spawn_chain_tasks(&task_runner, chain_id)?;

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

        let tx_id = if let Some(id) = req.tx_id {
            id
        } else {
            uuid::Uuid::new_v4().to_string()
        };

        let relayer = app
            .db
            .get_relayer(api_token.relayer_id())
            .await?
            .context("Missing relayer")?;

        if !relayer.enabled {
            return Err(poem::error::Error::from_string(
                "Relayer is disabled".to_string(),
                StatusCode::FORBIDDEN,
            ));
        }

        let relayer_queued_tx_count = app
            .db
            .get_relayer_pending_txs(api_token.relayer_id())
            .await?;

        if relayer_queued_tx_count > relayer.max_queued_txs as usize {
            return Err(poem::error::Error::from_string(
                "Relayer queue is full".to_string(),
                StatusCode::TOO_MANY_REQUESTS,
            ));
        }

        let relayer_queued_tx_gas_limit_sum = app
            .db
            .get_relayer_pending_txs_gas_limit_sum(api_token.relayer_id())
            .await?;

        let block_fees = app
            .db
            .get_latest_block_fees_by_chain_id(relayer.chain_id)
            .await?;
        if let Some(block_fees) = block_fees {
            let gas_limit = relayer_queued_tx_gas_limit_sum + req.gas_limit.0;
            let estimated_transactions_cost = block_fees.gas_price * gas_limit;

            // TODO: Cache?
            let http_provider: Provider<Http> =
                app.http_provider(relayer.chain_id).await?;

            let balance = http_provider
                .get_balance(relayer.address.0, None)
                .await
                .map_err(|err| {
                    eyre::eyre!("Error checking balance: {}", err)
                })?;

            if balance < estimated_transactions_cost {
                return Err(poem::error::Error::from_string(
                    "Relayer funds are insufficient for transaction to be mined.".to_string(),
                    StatusCode::UNPROCESSABLE_ENTITY,
                ));
            }
        }

        let res = app
            .db
            .create_transaction(
                &tx_id,
                req.to.0,
                req.data.as_ref().map(|d| &d.0[..]).unwrap_or(&[]),
                req.value.0,
                req.gas_limit.0,
                req.priority,
                req.blobs,
                api_token.relayer_id(),
            )
            .await?;

        if let CreateResult::CONFLICT = res {
            return Err(poem::error::Error::from_response(
                ErrorResponse::new(
                    StatusCode::CONFLICT,
                    "transaction_already_exists",
                    "Transaction with same id already exists.",
                ).into_response()
            ));
        }

        tracing::info!(tx_id, "Transaction created");

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
        let http_provider = app.http_provider(relayer_info.chain_id).await?;
        let url = http_provider.url();

        let response = reqwest::Client::new()
            .post(url.clone())
            .json(&req)
            .send()
            .await
            .map_err(|err| {
                eyre::eyre!("Error sending request to {}: {}", url, err)
            })?;

        let response: Value = response.json().await.map_err(|err| {
            eyre::eyre!("Error parsing response from {}: {}", url, err)
        })?;

        Ok(Json(response))
    }
}

fn default_false() -> bool {
    false
}

struct ServiceApi;

#[derive(ApiResponse)]
enum ServiceResponse {
    #[oai(status = 200)]
    Healthy,
}

#[OpenApi]
impl ServiceApi {
    /// Health
    #[oai(
        path = "/health",
        method = "get",
        operation_id = "health",
        tag = "OpenAPITags::Service"
    )]
    async fn health(&self) -> ServiceResponse {
        ServiceResponse::Healthy
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

pub async fn spawn_server(app: Arc<App>) -> eyre::Result<ServerHandle> {
    let api_service =
        create_api_service(app.config.server.server_address.as_ref());

    let router = Route::new()
        .nest("/rapidoc", api_service.rapidoc())
        .nest("/swagger", api_service.swagger_ui())
        .nest("/redoc", api_service.redoc())
        .nest("/schema.json", api_service.spec_endpoint())
        .nest("/schema.yml", api_service.spec_endpoint_yaml())
        .nest("/", api_service)
        .with(trace_middleware::TraceMiddleware)
        .with(Cors::new())
        .data(app.clone());

    let listener = TcpListener::bind(app.config.server.host);
    let acceptor = listener.into_acceptor().await?;

    let local_addrs = acceptor.local_addr();

    let server = poem::Server::new_with_acceptor(acceptor);

    let server_handle = tokio::spawn(async move {
        server.run(router).await?;
        Ok(())
    });

    Ok(ServerHandle {
        local_addrs,
        server_handle,
    })
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
