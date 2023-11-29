use std::sync::Arc;

use axum::extract::{Json, Path, State};
use axum::TypedHeader;
use ethers::types::{Address, Bytes, H256, U256};
use eyre::Result;
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::db::TxStatus;
use crate::server::middleware::AuthorizedRelayer;
use crate::server::ApiError;
use crate::types::TransactionPriority;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTxRequest {
    pub relayer_id: String,
    pub to: Address,
    #[serde(with = "crate::serde_utils::decimal_u256")]
    pub value: U256,
    #[serde(default)]
    pub data: Option<Bytes>,
    #[serde(with = "crate::serde_utils::decimal_u256")]
    pub gas_limit: U256,
    #[serde(default)]
    pub priority: TransactionPriority,
    #[serde(default)]
    pub tx_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTxResponse {
    pub tx_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTxResponse {
    pub tx_id: String,
    pub to: Address,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Bytes>,
    #[serde(with = "crate::serde_utils::decimal_u256")]
    pub value: U256,
    #[serde(with = "crate::serde_utils::decimal_u256")]
    pub gas_limit: U256,
    pub nonce: u64,

    // Sent tx data
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<H256>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TxStatus>,
}

pub async fn send_tx(
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
            req.priority,
            &req.relayer_id,
        )
        .await?;

    Ok(Json(SendTxResponse { tx_id }))
}

pub async fn get_tx(
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
