use std::sync::Arc;

use axum::routing::{get, post, IntoMakeService};
use axum::Router;
use hyper::server::conn::AddrIncoming;
use tower_http::validate_request::ValidateRequestHeaderLayer;

use self::routes::relayer::{
    create_relayer, create_relayer_api_key, get_relayer, get_relayers,
    purge_unsent_txs, relayer_rpc, update_relayer,
};
use self::routes::transaction::{get_tx, get_txs, send_tx};
use self::trace_layer::MatchedPathMakeSpan;
use crate::app::App;

mod error;
mod middleware;
pub mod routes;
mod trace_layer;

pub use self::error::ApiError;

pub async fn serve(app: Arc<App>) -> eyre::Result<()> {
    let server = spawn_server(app).await?;

    tracing::info!("Listening on {}", server.local_addr());

    server.await?;

    Ok(())
}

pub async fn spawn_server(
    app: Arc<App>,
) -> eyre::Result<axum::Server<AddrIncoming, IntoMakeService<Router>>> {
    let api_routes = Router::new()
        .route("/:api_token/tx", post(send_tx))
        .route("/:api_token/tx/:tx_id", get(get_tx))
        .route("/:api_token/txs", get(get_txs))
        .route("/:api_token/rpc", post(relayer_rpc))
        .with_state(app.clone());

    let mut admin_routes = Router::new()
        .route("/relayer", post(create_relayer))
        .route("/relayer/:relayer_id/reset", post(purge_unsent_txs))
        .route("/relayers", get(get_relayers))
        .route(
            "/relayer/:relayer_id",
            post(update_relayer).get(get_relayer),
        )
        .route("/relayer/:relayer_id/key", post(create_relayer_api_key))
        .route("/network/:chain_id", post(routes::network::create_network))
        .with_state(app.clone());

    if let Some((username, password)) = app.config.server.credentials() {
        admin_routes = admin_routes
            .layer(ValidateRequestHeaderLayer::basic(username, password));
    }

    let v1_routes = Router::new()
        .nest("/api", api_routes)
        .nest("/admin", admin_routes);

    let router = Router::new()
        .nest("/1", v1_routes)
        .route("/health", get(routes::health))
        .layer(
            tower_http::trace::TraceLayer::new_for_http()
                .make_span_with(MatchedPathMakeSpan),
        )
        .layer(axum::middleware::from_fn(middleware::log_response));

    let server = axum::Server::bind(&app.config.server.host)
        .serve(router.into_make_service());

    Ok(server)
}
