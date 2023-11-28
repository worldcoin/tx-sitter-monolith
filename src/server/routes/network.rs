use std::sync::Arc;

use axum::extract::{Json, Path, State};
use eyre::Result;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::app::App;
use crate::server::ApiError;
use crate::service::Service;
use crate::task_runner::TaskRunner;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewNetworkInfo {
    pub name: String,
    pub http_rpc: String,
    pub ws_rpc: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInfo {
    pub chain_id: u64,
    pub name: String,
    pub http_rpc: String,
    pub ws_rpc: String,
}

pub async fn create_network(
    State(app): State<Arc<App>>,
    Path(chain_id): Path<u64>,
    Json(network): Json<NewNetworkInfo>,
) -> Result<(), ApiError> {
    let http_url: Url = network.http_rpc.parse().map_err(|err| {
        tracing::error!("Invalid http rpc url: {}", err);
        ApiError::InvalidFormat
    })?;

    let ws_url: Url = network.ws_rpc.parse().map_err(|err| {
        tracing::error!("Invalid ws rpc url: {}", err);
        ApiError::InvalidFormat
    })?;

    app.db
        .create_network(
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

pub async fn _get_network(
    State(_app): State<Arc<App>>,
    Path(_chain_id): Path<u64>,
) -> &'static str {
    "Hello, World!"
}

pub async fn _get_networks(
    State(_app): State<Arc<App>>,
    Path(_chain_id): Path<u64>,
) -> &'static str {
    "Hello, World!"
}
