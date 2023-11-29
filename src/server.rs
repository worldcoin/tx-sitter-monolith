use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post, IntoMakeService};
use axum::Router;
use hyper::server::conn::AddrIncoming;
use thiserror::Error;

use self::routes::relayer::{create_relayer, get_relayer, update_relayer};
use self::routes::transaction::{get_tx, send_tx};
use crate::app::App;

mod middleware;
pub mod routes;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Invalid key encoding")]
    KeyEncoding,

    #[error("Invalid key length")]
    KeyLength,

    #[error("Unauthorized")]
    Unauthorized,

    #[error("Invalid format")]
    InvalidFormat,

    #[error("Missing tx")]
    MissingTx,

    #[error("Internal error {0}")]
    Eyre(#[from] eyre::Report),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status_code = match self {
            Self::KeyLength | Self::KeyEncoding => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Eyre(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::InvalidFormat => StatusCode::BAD_REQUEST,
            Self::MissingTx => StatusCode::NOT_FOUND,
        };

        let message = self.to_string();

        (status_code, message).into_response()
    }
}

pub async fn serve(app: Arc<App>) -> eyre::Result<()> {
    let server = spawn_server(app).await?;

    tracing::info!("Listening on {}", server.local_addr());

    server.await?;

    Ok(())
}

pub async fn spawn_server(
    app: Arc<App>,
) -> eyre::Result<axum::Server<AddrIncoming, IntoMakeService<Router>>> {
    let tx_routes = Router::new()
        .route("/send", post(send_tx))
        .route("/:tx_id", get(get_tx))
        .layer(axum::middleware::from_fn_with_state(
            app.clone(),
            middleware::auth,
        ))
        .with_state(app.clone());

    let relayer_routes = Router::new()
        .route("/", post(create_relayer))
        .route("/:relayer_id", post(update_relayer))
        .route("/:relayer_id", get(get_relayer))
        .with_state(app.clone());

    let network_routes = Router::new()
        // .route("/", get(routes::network::get_networks))
        // .route("/:chain_id", get(routes::network::get_network))
        .route("/:chain_id", post(routes::network::create_network))
        .with_state(app.clone());

    let v1_routes = Router::new()
        .nest("/tx", tx_routes)
        .nest("/relayer", relayer_routes)
        .nest("/network", network_routes);

    let router = Router::new()
        .nest("/1", v1_routes)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(middleware::log_response));

    let server = axum::Server::bind(&app.config.server.host)
        .serve(router.into_make_service());

    Ok(server)
}
