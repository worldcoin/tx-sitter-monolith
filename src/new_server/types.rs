use poem_openapi::Object;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Default, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct NewNetworkInfo {
    pub name: String,
    pub http_rpc: String,
    pub ws_rpc: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, FromRow, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct NetworkInfo {
    #[sqlx(try_from = "i64")]
    pub chain_id: u64,
    pub name: String,
    pub http_rpc: String,
    pub ws_rpc: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct CreateRelayerRequest {
    /// New relayer name
    pub name: String,
    /// The chain id of the relayer
    pub chain_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct CreateRelayerResponse {
    /// ID of the created relayer
    pub relayer_id: String,
    // TODO: Make type safe
    /// Address of the created relayer
    ///
    /// Hex encoded, example "0x1234...5678"
    pub address: String,
}
