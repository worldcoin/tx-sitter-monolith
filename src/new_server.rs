use std::net::SocketAddr;
use std::sync::Arc;

use ethers::signers::Signer;
use poem::http::StatusCode;
use poem::listener::{Acceptor, Listener, TcpListener};
use poem::web::{Data, LocalAddr};
use poem::{EndpointExt, Result, Route};
use poem_openapi::param::Path;
use poem_openapi::payload::{Json, PlainText};
use poem_openapi::{ApiResponse, OpenApi, OpenApiService};
use url::Url;

use crate::api_key::ApiKey;
use crate::app::App;
use crate::server::routes::relayer::CreateApiKeyResponse;
use crate::service::Service;
use crate::task_runner::TaskRunner;
use crate::types::{
    CreateRelayerRequest, CreateRelayerResponse, NetworkInfo, NewNetworkInfo,
    RelayerInfo, RelayerUpdate,
};

struct AdminApi;

#[derive(ApiResponse)]
enum AdminResponse {
    #[oai(status = 200)]
    RelayerCreated(Json<CreateRelayerResponse>),

    #[oai(status = 200)]
    NetworkCreated,

    #[oai(status = 500)]
    InternalServerError(PlainText<String>),
}

#[OpenApi(prefix_path = "/1/admin/")]
impl AdminApi {
    /// Create Relayer
    #[oai(path = "/relayer", method = "post")]
    async fn create_relayer(
        &self,
        app: Data<&Arc<App>>,
        req: Json<CreateRelayerRequest>,
    ) -> AdminResponse {
        let (key_id, signer) = match app.keys_source.new_signer(&req.name).await
        {
            Ok(signer) => signer,
            Err(e) => {
                tracing::error!("Failed to create signer: {:?}", e);
                return AdminResponse::InternalServerError(PlainText(
                    "Failed to create signer".to_string(),
                ));
            }
        };

        let address = signer.address();

        let relayer_id = uuid::Uuid::new_v4();
        let relayer_id = relayer_id.to_string();

        let result = app
            .db
            .create_relayer(
                &relayer_id,
                &req.name,
                req.chain_id,
                &key_id,
                address,
            )
            .await;

        match result {
            Ok(()) => {}
            Err(e) => {
                tracing::error!("Failed to create relayer: {:?}", e);
                return AdminResponse::InternalServerError(PlainText(
                    "Failed to create relayer".to_string(),
                ));
            }
        }

        AdminResponse::RelayerCreated(Json(CreateRelayerResponse {
            relayer_id,
            address: address.into(),
        }))
    }

    /// Get Relayers
    #[oai(path = "/relayers", method = "get")]
    async fn get_relayers(
        &self,
        app: Data<&Arc<App>>,
    ) -> Result<Json<Vec<RelayerInfo>>> {
        let relayer_info = app.db.get_relayers().await?;

        Ok(Json(relayer_info))
    }

    /// Get Relayer
    #[oai(path = "/relayer/:relayer_id", method = "get")]
    async fn get_relayer(
        &self,
        app: Data<&Arc<App>>,
        Path(relayer_id): Path<String>,
    ) -> Result<Json<RelayerInfo>> {
        let relayer_info =
            app.db.get_relayer(&relayer_id).await.map_err(|err| {
                poem::error::Error::from_string(
                    err.to_string(),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            })?;

        Ok(Json(relayer_info))
    }

    /// Update Relayer
    #[oai(path = "/relayer/:relayer_id", method = "post")]
    async fn update_relayer(
        &self,
        app: Data<&Arc<App>>,
        Path(relayer_id): Path<String>,
        Json(req): Json<RelayerUpdate>,
    ) -> Result<()> {
        app.db
            .update_relayer(&relayer_id, &req)
            .await
            .map_err(|err| {
                poem::error::Error::from_string(
                    err.to_string(),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            })?;

        Ok(())
    }

    /// Reset Relayer transactions
    ///
    /// Purges unsent transactions, useful for unstucking the relayer
    #[oai(path = "/relayer/:relayer_id/reset", method = "post")]
    async fn purge_unsent_txs(
        &self,
        app: Data<&Arc<App>>,
        Path(relayer_id): Path<String>,
    ) -> Result<()> {
        app.db.purge_unsent_txs(&relayer_id).await.map_err(|err| {
            poem::error::Error::from_string(
                err.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;

        Ok(())
    }

    /// Create Relayer API Key
    #[oai(path = "/relayer/:relayer_id/key", method = "post")]
    async fn create_relayer_api_key(
        &self,
        app: Data<&Arc<App>>,
        Path(relayer_id): Path<String>,
    ) -> Result<Json<CreateApiKeyResponse>> {
        let api_key = ApiKey::random(&relayer_id);

        app.db
            .create_api_key(&relayer_id, api_key.api_key_secret_hash())
            .await?;

        Ok(Json(CreateApiKeyResponse { api_key }))
    }

    /// Create Network
    #[oai(path = "/network/:chain_id", method = "post")]
    async fn create_network(
        &self,
        app: Data<&Arc<App>>,
        Path(chain_id): Path<u64>,
        Json(network): Json<NewNetworkInfo>,
    ) -> Result<()> {
        let http_url: Url = network
            .http_rpc
            .parse::<Url>()
            .map_err(|err| poem::error::BadRequest(err))?;

        let ws_url: Url = network
            .ws_rpc
            .parse::<Url>()
            .map_err(|err| poem::error::BadRequest(err))?;

        app.db
            .upsert_network(
                chain_id,
                &network.name,
                http_url.as_str(),
                ws_url.as_str(),
            )
            .await
            .map_err(|err| {
                poem::error::Error::from_string(
                    err.to_string(),
                    StatusCode::INTERNAL_SERVER_ERROR,
                )
            })?;

        let task_runner = TaskRunner::new(app.clone());
        Service::spawn_chain_tasks(&task_runner, chain_id).map_err(|err| {
            poem::error::Error::from_string(
                err.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;

        Ok(())
    }

    /// Get Networks
    #[oai(path = "/networks", method = "get")]
    async fn list_networks(
        &self,
        app: Data<&Arc<App>>,
    ) -> Result<Json<Vec<NetworkInfo>>> {
        let networks = app.db.get_networks().await.map_err(|err| {
            poem::error::Error::from_string(
                err.to_string(),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;

        Ok(Json(networks))
    }
}

struct ConsumerApi;

#[OpenApi(prefix_path = "/1/api/")]
impl ConsumerApi {}

struct ServiceApi;

#[derive(ApiResponse)]
enum ServiceResponse {
    #[oai(status = 200)]
    Healthy,
}

#[OpenApi]
impl ServiceApi {
    #[oai(path = "/", method = "get")]
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

pub async fn spawn_server(app: Arc<App>) -> eyre::Result<ServerHandle> {
    let mut api_service = OpenApiService::new(
        (AdminApi, ConsumerApi, ServiceApi),
        "Tx Sitter",
        version::version!(),
    );

    if let Some(server_address) = app.config.server.server_address.as_ref() {
        api_service = api_service.server(server_address.clone());
    }

    let router = Route::new()
        .nest("/explorer", api_service.rapidoc())
        .nest("/schema.json", api_service.spec_endpoint())
        .nest("/schema.yml", api_service.spec_endpoint_yaml())
        .nest("/", api_service)
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
