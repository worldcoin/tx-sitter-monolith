use std::sync::Arc;

use axum::extract::{Json, Path, State};
use ethers::signers::Signer;
use ethers::types::Address;
use eyre::Result;
use serde::{Deserialize, Serialize};

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

pub async fn update_relayer(
    State(app): State<Arc<App>>,
    Path(relayer_id): Path<String>,
    Json(req): Json<RelayerUpdate>,
) -> Result<(), ApiError> {
    app.db.update_relayer(&relayer_id, &req).await?;

    Ok(())
}

pub async fn get_relayer(
    State(app): State<Arc<App>>,
    Path(relayer_id): Path<String>,
) -> Result<Json<RelayerInfo>, ApiError> {
    let relayer_info = app.db.get_relayer(&relayer_id).await?;

    Ok(Json(relayer_info))
}
