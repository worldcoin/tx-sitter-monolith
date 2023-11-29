use std::sync::Arc;

use axum::extract::{Json, Path, State};
use ethers::signers::Signer;
use ethers::types::Address;
use eyre::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app::App;
use crate::server::ApiError;
use crate::types::{RelayerInfo, RelayerUpdate};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRelayerRequest {
    pub name: String,
    pub chain_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRelayerResponse {
    pub relayer_id: String,
    pub address: Address,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcRequest {
    pub id: i32,
    pub method: String,
    #[serde(default)]
    pub params: Value,
    pub jsonrpc: JsonRpcVersion,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcResponse {
    pub id: i32,
    pub result: Value,
    pub jsonrpc: JsonRpcVersion,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum JsonRpcVersion {
    #[serde(rename = "2.0")]
    V2,
}

#[tracing::instrument(skip(app))]
pub async fn create_relayer(
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

#[tracing::instrument(skip(app))]
pub async fn update_relayer(
    State(app): State<Arc<App>>,
    Path(relayer_id): Path<String>,
    Json(req): Json<RelayerUpdate>,
) -> Result<(), ApiError> {
    app.db.update_relayer(&relayer_id, &req).await?;

    Ok(())
}

#[tracing::instrument(skip(app))]
pub async fn get_relayer(
    State(app): State<Arc<App>>,
    Path(relayer_id): Path<String>,
) -> Result<Json<RelayerInfo>, ApiError> {
    let relayer_info = app.db.get_relayer(&relayer_id).await?;

    Ok(Json(relayer_info))
}

#[tracing::instrument(skip(app))]
pub async fn relayer_rpc(
    State(app): State<Arc<App>>,
    Path(relayer_id): Path<String>,
    Json(req): Json<RpcRequest>,
) -> Result<Json<Value>, ApiError> {
    let relayer_info = app.db.get_relayer(&relayer_id).await?;

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
