use std::net::SocketAddr;
use std::sync::Arc;

use ethers::signers::Signer;
use poem::listener::{Acceptor, Listener, TcpListener};
use poem::web::{Data, LocalAddr};
use poem::{EndpointExt, Route};
use poem_openapi::payload::{Json, PlainText};
use poem_openapi::{ApiResponse, OpenApi, OpenApiService};

use crate::app::App;

mod types;

struct AdminApi;

#[derive(ApiResponse)]
enum CreateRelayerResponse {
    #[oai(status = 200)]
    RelayerCreated(Json<types::CreateRelayerResponse>),

    #[oai(status = 500)]
    InternalServerError(PlainText<String>),
}

#[OpenApi(prefix_path = "/1/admin/")]
impl AdminApi {
    #[oai(path = "/relayer", method = "post")]
    async fn create_relayer(
        &self,
        app: Data<&Arc<App>>,
        req: Json<types::CreateRelayerRequest>,
    ) -> CreateRelayerResponse {
        let (key_id, signer) = match app.keys_source.new_signer(&req.name).await
        {
            Ok(signer) => signer,
            Err(e) => {
                tracing::error!("Failed to create signer: {:?}", e);
                return CreateRelayerResponse::InternalServerError(PlainText(
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
                return CreateRelayerResponse::InternalServerError(PlainText(
                    "Failed to create relayer".to_string(),
                ));
            }
        }

        CreateRelayerResponse::RelayerCreated(Json(
            types::CreateRelayerResponse {
                relayer_id,
                address: format!("{address:?}"),
            },
        ))
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
