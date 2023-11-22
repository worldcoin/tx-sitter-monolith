use std::sync::Arc;

use axum::extract::{Json, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post, IntoMakeService};
use axum::{Router, TypedHeader};
use ethers_signers::Signer;
use eyre::Result;
use hyper::server::conn::AddrIncoming;
use middleware::AuthorizedRelayer;
use thiserror::Error;

use self::data::{
    CreateRelayerRequest, CreateRelayerResponse, GetTxResponse, SendTxRequest,
    SendTxResponse,
};
use crate::app::App;

pub mod data;
mod middleware;

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

async fn send_tx(
    State(app): State<Arc<App>>,
    TypedHeader(authorized_relayer): TypedHeader<AuthorizedRelayer>,
    Json(req): Json<SendTxRequest>,
) -> Result<Json<SendTxResponse>, ApiError> {
    if !authorized_relayer.is_authorized(&req.relayer_id) {
        return Err(ApiError::Unauthorized);
    }

    let tx_id = if let Some(id) = req.tx_id {
        id
    } else {
        uuid::Uuid::new_v4().to_string()
    };

    app.db
        .create_transaction(
            &tx_id,
            req.to,
            req.data.as_ref().map(|d| &d[..]).unwrap_or(&[]),
            req.value,
            req.gas_limit,
            &req.relayer_id,
        )
        .await?;

    Ok(Json(SendTxResponse { tx_id }))
}

async fn get_tx(
    State(app): State<Arc<App>>,
    Path(tx_id): Path<String>,
) -> Result<Json<GetTxResponse>, ApiError> {
    let tx = app.db.read_tx(&tx_id).await?.ok_or(ApiError::MissingTx)?;

    let get_tx_response = GetTxResponse {
        tx_id: tx.tx_id,
        to: tx.to.0,
        data: if tx.data.is_empty() {
            None
        } else {
            Some(tx.data.into())
        },
        value: tx.value.0,
        gas_limit: tx.gas_limit.0,
        nonce: tx.nonce,
        tx_hash: tx.tx_hash.map(|h| h.0),
        status: tx.status,
    };

    Ok(Json(get_tx_response))
}

async fn create_relayer(
    State(app): State<Arc<App>>,
    Json(req): Json<CreateRelayerRequest>,
) -> Result<Json<CreateRelayerResponse>, ApiError> {
    let (key_id, signer) = app.keys_source.new_signer().await?;

    let address = signer.address();

    let relayer_id = uuid::Uuid::new_v4();
    let relayer_id = relayer_id.to_string();

    app.db
        .create_relayer(&relayer_id, &req.name, req.chain_id, &key_id, address)
        .await?;

    Ok(Json(CreateRelayerResponse {
        relayer_id,
        address,
    }))
}

async fn get_relayer(
    State(_app): State<Arc<App>>,
    Path(_relayer_id): Path<String>,
) -> &'static str {
    "Hello, World!"
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
        .route("/create", post(create_relayer))
        .route("/:relayer_id", get(get_relayer))
        .with_state(app.clone());

    // let network_routes = Router::new()
    //     .route("/");

    let router = Router::new()
        .nest("/1/tx", tx_routes)
        .nest("/1/relayer", relayer_routes)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(middleware::log_response));

    let server = axum::Server::bind(&app.config.server.host)
        .serve(router.into_make_service());

    Ok(server)
}
