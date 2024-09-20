use base_api_types::{Address, DecimalU256, HexBytes, H256};
use poem_openapi::{Enum, Object};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api_key::ApiKey;
use crate::db::data::{NetworkInfo, RelayerGasPriceLimit, RelayerInfo};

#[derive(
    Deserialize, Serialize, Debug, Clone, Copy, Default, sqlx::Type, Enum,
)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
#[sqlx(type_name = "transaction_priority", rename_all = "camelCase")]
pub enum TransactionPriority {
    // 5th percentile
    Slowest = 0,
    // 25th percentile
    Slow = 1,
    // 50th percentile
    #[default]
    Regular = 2,
    // 75th percentile
    Fast = 3,
    // 95th percentile
    Fastest = 4,
}

impl TransactionPriority {
    pub fn to_percentile_index(self) -> usize {
        self as usize
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct RelayerResponse {
    pub id: String,
    pub name: String,
    pub chain_id: u64,
    pub key_id: String,
    pub address: Address,
    pub nonce: u64,
    pub current_nonce: u64,
    pub max_inflight_txs: u64,
    pub max_queued_txs: u64,
    pub gas_price_limits: Vec<RelayerGasPriceLimitResponse>,
    pub enabled: bool,
}

impl From<RelayerInfo> for RelayerResponse {
    fn from(value: RelayerInfo) -> Self {
        Self {
            id: value.id,
            name: value.name,
            chain_id: value.chain_id,
            key_id: value.key_id,
            address: value.address.into(),
            nonce: value.nonce,
            current_nonce: value.current_nonce,
            max_inflight_txs: value.max_inflight_txs,
            max_queued_txs: value.max_queued_txs,
            gas_price_limits: value
                .gas_price_limits
                .into_iter()
                .map(|v| v.into())
                .collect(),
            enabled: value.enabled,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct RelayerUpdateRequest {
    #[serde(default)]
    pub relayer_name: Option<String>,
    #[serde(default)]
    pub max_inflight_txs: Option<u64>,
    #[serde(default)]
    pub max_queued_txs: Option<u64>,
    #[serde(default)]
    pub gas_price_limits: Option<Vec<RelayerGasPriceLimitResponse>>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct RelayerGasPriceLimitResponse {
    pub value: DecimalU256,
    pub chain_id: i64,
}

impl From<RelayerGasPriceLimit> for RelayerGasPriceLimitResponse {
    fn from(value: RelayerGasPriceLimit) -> Self {
        Self {
            value: value.value.into(),
            chain_id: value.chain_id,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct CreateApiKeyResponse {
    pub api_key: ApiKey,
}

#[derive(Debug, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct CreateNetworkRequest {
    pub name: String,
    pub http_rpc: String,
    pub ws_rpc: String,
}

#[derive(Debug, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct NetworkResponse {
    pub chain_id: u64,
    pub name: String,
    pub http_rpc: String,
    pub ws_rpc: String,
}

impl From<NetworkInfo> for NetworkResponse {
    fn from(value: NetworkInfo) -> Self {
        Self {
            chain_id: value.chain_id,
            name: value.name,
            http_rpc: value.http_rpc,
            ws_rpc: value.ws_rpc,
        }
    }
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
    /// Address of the created relayer
    pub address: Address,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct SendTxRequest {
    pub to: Address,
    /// Transaction value
    pub value: DecimalU256,
    #[serde(default)]
    #[oai(default)]
    pub data: Option<HexBytes>,
    /// Transaction gas limit
    pub gas_limit: DecimalU256,
    /// Transaction priority
    ///
    /// The values map to the following percentiles:
    ///
    /// slowest -> 5th percentile
    ///
    /// slow -> 25th percentile
    ///
    /// regular -> 50th percentile
    ///
    /// fast -> 75th percentile
    ///
    /// fastest -> 95th percentile
    ///
    /// i.e. a transaction with priority `fast` will have a gas price that is higher than 75% of the gas prices of other transactions (based on fee estimates from previous blocks).
    #[serde(default)]
    #[oai(default)]
    pub priority: TransactionPriority,
    /// An optional transaction id. If not provided tx-sitter will generate a UUID.
    ///
    /// Can be used to provide idempotency for the transaction.
    #[serde(default)]
    #[oai(default)]
    pub tx_id: Option<String>,
    // TODO: poem_openapi thinks this is a nested array of numbers
    #[serde(default, with = "crate::serde_utils::base64_binary")]
    #[oai(default)]
    pub blobs: Option<Vec<Vec<u8>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct SendTxResponse {
    pub tx_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GetTxResponse {
    pub tx_id: String,
    pub to: Address,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<HexBytes>,
    pub value: DecimalU256,
    pub gas_limit: DecimalU256,
    pub nonce: u64,

    // Sent tx data
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<H256>,
    #[serde(default)]
    #[oai(default)]
    pub status: Option<TxStatus>,
}

#[derive(
    Debug, Clone, Serialize, Deserialize, Copy, PartialEq, Eq, sqlx::Type, Enum,
)]
#[sqlx(rename_all = "camelCase")]
#[sqlx(type_name = "tx_status")]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub enum TxStatus {
    Pending,
    Mined,
    Finalized,
}

#[derive(Debug, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct RpcRequest {
    pub id: i32,
    pub method: String,
    #[serde(default)]
    #[oai(default)]
    pub params: Value,
    pub jsonrpc: JsonRpcVersion,
}

#[derive(Debug, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct RpcResponse {
    pub id: i32,
    pub result: Value,
    pub jsonrpc: JsonRpcVersion,
}

#[derive(Debug, Serialize, Deserialize, Enum)]
pub enum JsonRpcVersion {
    #[serde(rename = "2.0")]
    #[oai(rename = "2.0")]
    V2,
}

impl TxStatus {
    pub fn previous(self) -> Self {
        match self {
            Self::Pending => Self::Pending,
            Self::Mined => Self::Pending,
            Self::Finalized => Self::Mined,
        }
    }
}

#[cfg(test)]
mod tests {
    use ethers::types::{Address, U256};
    use ethers::utils::parse_units;

    use super::*;

    #[test]
    fn relayer_response_serialize() {
        let info = RelayerResponse {
            id: "id".to_string(),
            name: "name".to_string(),
            chain_id: 1,
            key_id: "key_id".to_string(),
            address: Address::zero().into(),
            nonce: 0,
            current_nonce: 0,
            max_inflight_txs: 0,
            max_queued_txs: 0,
            gas_price_limits: vec![RelayerGasPriceLimitResponse {
                value: U256::zero().into(),
                chain_id: 1,
            }],
            enabled: true,
        };

        let json = serde_json::to_string_pretty(&info).unwrap();

        let expected = indoc::indoc! {r#"
            {
              "id": "id",
              "name": "name",
              "chainId": 1,
              "keyId": "key_id",
              "address": "0x0000000000000000000000000000000000000000",
              "nonce": 0,
              "currentNonce": 0,
              "maxInflightTxs": 0,
              "maxQueuedTxs": 0,
              "gasPriceLimits": [
                {
                  "value": "0",
                  "chainId": 1
                }
              ],
              "enabled": true
            }
        "#};

        assert_eq!(json.trim(), expected.trim());
    }

    #[test]
    fn send_tx_request() {
        let value: U256 = parse_units("1", "ether").unwrap().into();

        let request = SendTxRequest {
            to: Address(Address::zero()),
            value: value.into(),
            data: Some(HexBytes::from(vec![0])),
            gas_limit: U256::zero().into(),
            priority: TransactionPriority::Regular,
            tx_id: Some("tx_id".to_string()),
            blobs: Some(vec![vec![0]]),
        };

        let json = serde_json::to_string_pretty(&request).unwrap();

        let expected = indoc::indoc! {r#"
            {
              "to": "0x0000000000000000000000000000000000000000",
              "value": "1000000000000000000",
              "data": "0x00",
              "gasLimit": "0",
              "priority": "regular",
              "txId": "tx_id",
              "blobs": [
                "AA=="
              ]
            }
        "#};

        assert_eq!(json.trim(), expected.trim());
    }
}
