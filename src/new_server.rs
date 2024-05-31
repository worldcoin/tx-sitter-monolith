use std::net::SocketAddr;
use std::sync::Arc;

use poem::listener::{Acceptor, Listener, TcpListener};
use poem::web::LocalAddr;
use poem::Route;
use poem_openapi::{ApiResponse, OpenApi, OpenApiService};

use crate::app::App;

struct AdminApi;

#[OpenApi]
impl AdminApi {}

struct ConsumerApi;

#[OpenApi]
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
        .nest("/", api_service);

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
