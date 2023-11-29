use ethers::types::{Address, Bytes, H256, U256};
use serde::{Deserialize, Serialize};

use crate::db::TxStatus;
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
