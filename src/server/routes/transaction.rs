use std::sync::Arc;

use axum::extract::{Json, Path, Query, State};
use ethers::types::{Address, Bytes, H256, U256};
use eyre::Result;
use serde::{Deserialize, Serialize};

use crate::api_key::ApiKey;
use crate::app::App;
use crate::db::TxStatus;
use crate::server::ApiError;
use crate::types::TransactionPriority;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTxRequest {
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
pub struct GetTxQuery {
    #[serde(default)]
    pub status: Option<GetTxResponseStatus>,
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
    pub status: GetTxResponseStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
#[serde(rename_all = "camelCase")]
pub enum GetTxResponseStatus {
    TxStatus(TxStatus),
    Unsent(UnsentStatus),
}

// We need this status as a separate enum to avoid manual serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UnsentStatus {
    Unsent,
}

#[tracing::instrument(skip(app))]
pub async fn send_tx(
    State(app): State<Arc<App>>,
    Path(api_token): Path<ApiKey>,
    Json(req): Json<SendTxRequest>,
) -> Result<Json<SendTxResponse>, ApiError> {
    if !app.is_authorized(&api_token).await? {
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
            api_token.relayer_id(),
        )
        .await?;

    tracing::info!(tx_id, "Transaction created");

    Ok(Json(SendTxResponse { tx_id }))
}

#[tracing::instrument(skip(app, api_token))]
pub async fn get_txs(
    State(app): State<Arc<App>>,
    Path(api_token): Path<ApiKey>,
    Query(query): Query<GetTxQuery>,
) -> Result<Json<Vec<GetTxResponse>>, ApiError> {
    if !app.is_authorized(&api_token).await? {
        return Err(ApiError::Unauthorized);
    }

    let txs = match query.status {
        Some(GetTxResponseStatus::TxStatus(status)) => {
            app.db
                .read_txs(api_token.relayer_id(), Some(Some(status)))
                .await?
        }
        Some(GetTxResponseStatus::Unsent(_)) => {
            app.db.read_txs(api_token.relayer_id(), Some(None)).await?
        }
        None => app.db.read_txs(api_token.relayer_id(), None).await?,
    };

    let txs =
        txs.into_iter()
            .map(|tx| GetTxResponse {
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
                status: tx.status.map(GetTxResponseStatus::TxStatus).unwrap_or(
                    GetTxResponseStatus::Unsent(UnsentStatus::Unsent),
                ),
            })
            .collect();

    Ok(Json(txs))
}

#[tracing::instrument(skip(app))]
pub async fn get_tx(
    State(app): State<Arc<App>>,
    Path((api_token, tx_id)): Path<(ApiKey, String)>,
) -> Result<Json<GetTxResponse>, ApiError> {
    if !app.is_authorized(&api_token).await? {
        return Err(ApiError::Unauthorized);
    }

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
        status: tx
            .status
            .map(GetTxResponseStatus::TxStatus)
            .unwrap_or(GetTxResponseStatus::Unsent(UnsentStatus::Unsent)),
    };

    Ok(Json(get_tx_response))
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case(GetTxResponseStatus::TxStatus(TxStatus::Pending) => "pending")]
    #[test_case(GetTxResponseStatus::Unsent(UnsentStatus::Unsent) => "unsent")]
    fn get_tx_response_status_serialization(
        status: GetTxResponseStatus,
    ) -> &'static str {
        let json = serde_json::to_string(&status).unwrap();

        let s = json.trim_start_matches('\"').trim_end_matches('\"');

        Box::leak(s.to_owned().into_boxed_str())
    }
}
