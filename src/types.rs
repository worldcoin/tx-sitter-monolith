use alloy::primitives::keccak256;
use base_api_types::{Address, DecimalU256, HexBytes, H256};
use chrono::{DateTime, Utc};
use poem::http::{header, StatusCode};
use poem::{IntoResponse, Response};
use poem_openapi::{Enum, Object};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::api_key::ApiKey;
use crate::db::data::{
    NetworkInfo, ReadTxData, RelayerGasPriceLimit, RelayerInfo, TxEventData,
};
use crate::tx_events::EventCursor;

pub mod secret_string;
pub mod secret_url;
pub mod tx_metadata;

pub use self::tx_metadata::TxMetadata;

#[derive(
    Deserialize,
    Serialize,
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    sqlx::Type,
    Enum,
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

// serde parses v2 requests, so that `metadata` keeps its exact text. The
// `oai` attributes only describe the schema and must match the serde ones.
#[derive(Debug, Clone, Deserialize, Object)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[oai(rename_all = "camelCase", deny_unknown_fields)]
pub struct SendTxRequestV2 {
    pub to: Address,
    /// Transaction value
    pub value: DecimalU256,
    #[serde(default)]
    #[oai(default)]
    pub data: Option<HexBytes>,
    /// Transaction gas limit
    pub gas_limit: DecimalU256,
    /// Transaction priority, as in the v1 API. Defaults to `regular`.
    #[serde(default)]
    #[oai(default)]
    pub priority: TransactionPriority,
    /// An optional transaction id. If not provided tx-sitter will generate a UUID.
    ///
    /// Sending an id again with an identical request returns the existing
    /// transaction and changes nothing. Requests are identical when their
    /// fields are equal and their `metadata` is the same JSON text, byte for
    /// byte, as a retry that re-sends the same body is. Sending the id with a
    /// different request, or reusing an id sent through the v1 API, returns
    /// 409.
    ///
    /// An id stays taken while its transaction is kept: tx-sitter deletes
    /// finalized transactions `service.tx_retention` (30 days by default)
    /// after they are mined. After that a repeat creates a new transaction.
    #[serde(default, deserialize_with = "deserialize_tx_id")]
    #[oai(
        default,
        validator(min_length = 1, max_length = 255, pattern = r"^[^\x00]*$")
    )]
    pub tx_id: Option<String>,
    #[serde(default)]
    #[oai(default)]
    pub metadata: Option<TxMetadata>,
}

/// Reads a `txId` of 1 to 255 characters without NUL, which is what the
/// validator of [`SendTxRequestV2::tx_id`] declares
fn deserialize_tx_id<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let tx_id = Option::<String>::deserialize(deserializer)?;

    if let Some(tx_id) = &tx_id {
        let length = tx_id.chars().count();

        if !(1..=255).contains(&length) || tx_id.contains('\0') {
            return Err(serde::de::Error::custom(
                "txId must be 1 to 255 characters without NUL",
            ));
        }
    }

    Ok(tx_id)
}

/// Identifies a send request in idempotency checks.
///
/// Two requests hash the same exactly when they would create the same
/// transaction: absent `data` equals empty data, absent `priority` equals
/// `regular`, and `metadata` is compared as JSON text. `txId` isn't part of
/// the hash, because it is what a repeat is looked up by.
///
/// Hashes are stored with transactions and compared across versions, so a
/// change to this encoding makes retries of earlier transactions conflict.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CanonicalSendTx<'a> {
    relayer_id: &'a str,
    to: String,
    value: String,
    data: String,
    gas_limit: String,
    priority: TransactionPriority,
    metadata: Option<&'a TxMetadata>,
}

impl SendTxRequestV2 {
    /// Returns the hash that identifies this request when sent by relayer
    /// `relayer_id`, see [`CanonicalSendTx`].
    pub fn request_hash(&self, relayer_id: &str) -> [u8; 32] {
        let canonical = CanonicalSendTx {
            relayer_id,
            to: hex::encode(self.to.0),
            value: self.value.0.to_string(),
            data: self
                .data
                .as_ref()
                .map(|data| hex::encode(&data.0))
                .unwrap_or_default(),
            gas_limit: self.gas_limit.0.to_string(),
            priority: self.priority,
            metadata: self.metadata.as_ref(),
        };

        let encoded = serde_json::to_vec(&canonical)
            .expect("Serializing strings and JSON text can't fail");

        keccak256(encoded).0
    }
}

// serde writes v2 responses, so that `metadata` comes back exactly as it was
// sent
#[derive(Debug, Clone, Serialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GetTxResponseV2 {
    pub tx_id: String,
    pub relayer_id: String,
    pub chain_id: u64,
    pub to: Address,
    pub data: HexBytes,
    pub value: DecimalU256,
    pub gas_limit: DecimalU256,
    pub nonce: u64,
    pub priority: TransactionPriority,
    /// `null` for transactions sent through the v1 API
    pub metadata: Option<TxMetadata>,
    /// When tx-sitter stored the transaction
    pub created_at: DateTime<Utc>,
    pub status: TxStatusV2,
    /// Hash of the broadcast transaction that is mined, or else of the latest
    /// one broadcast
    pub tx_hash: Option<H256>,
    /// Timestamp of the block the transaction was mined in
    pub mined_at: Option<DateTime<Utc>>,
}

impl From<ReadTxData> for GetTxResponseV2 {
    fn from(tx: ReadTxData) -> Self {
        Self {
            tx_id: tx.tx_id,
            relayer_id: tx.relayer_id,
            chain_id: tx.chain_id,
            to: tx.to.into(),
            data: tx.data.into(),
            value: tx.value.into(),
            gas_limit: tx.gas_limit.into(),
            nonce: tx.nonce,
            priority: tx.priority,
            metadata: tx.metadata,
            created_at: tx.created_at,
            status: tx.status.into(),
            tx_hash: tx.tx_hash.map(|v| v.into()),
            mined_at: tx.mined_at,
        }
    }
}

/// Transaction status
///
/// queued -> not broadcast yet
///
/// pending -> broadcast, not mined yet
///
/// mined -> included in a block
///
/// finalized -> included in a block old enough to count as final
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Enum)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub enum TxStatusV2 {
    Queued,
    Pending,
    Mined,
    Finalized,
}

impl From<Option<TxStatus>> for TxStatusV2 {
    fn from(status: Option<TxStatus>) -> Self {
        match status {
            None => Self::Queued,
            Some(TxStatus::Pending) => Self::Pending,
            Some(TxStatus::Mined) => Self::Mined,
            Some(TxStatus::Finalized) => Self::Finalized,
        }
    }
}

// serde writes v2 responses, so that `metadata` comes back exactly as it was
// sent
#[derive(Debug, Clone, Serialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct GetTxEventsResponse {
    /// The relayer's events after `after`, in the order they happened
    pub events: Vec<TxEvent>,
    /// Pass as `after` to get the events after these. It only works in the
    /// relayer's feed.
    pub next_cursor: String,
}

impl GetTxEventsResponse {
    /// Answers a request of relayer `relayer_id` with `events`, which
    /// `next_cursor` follows
    pub fn new(
        events: Vec<TxEventData>,
        next_cursor: EventCursor,
        relayer_id: &str,
    ) -> Self {
        Self {
            events: events.into_iter().map(TxEvent::from).collect(),
            next_cursor: next_cursor.encode(relayer_id),
        }
    }
}

/// A status change of a transaction
#[derive(Debug, Clone, Serialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct TxEvent {
    pub tx_id: String,
    pub relayer_id: String,
    pub chain_id: u64,
    /// The status the transaction changed to: `pending` once it is
    /// broadcast, escalated or reorged out of its block, `mined` once it is
    /// in a block or reorged into another one, `finalized` once its block is
    /// old enough to count as final
    pub status: TxStatus,
    /// Hash of the broadcast transaction that is mined, or else of the latest
    /// one broadcast
    pub tx_hash: Option<H256>,
    /// `null` for transactions sent through the v1 API, and once the
    /// transaction is deleted `service.tx_retention` after it is mined
    pub metadata: Option<TxMetadata>,
    /// When tx-sitter stored the change
    pub created_at: DateTime<Utc>,
}

impl From<TxEventData> for TxEvent {
    fn from(event: TxEventData) -> Self {
        Self {
            tx_id: event.tx_id,
            relayer_id: event.relayer_id,
            chain_id: event.chain_id,
            status: event.status,
            tx_hash: event.tx_hash.map(|v| v.into()),
            metadata: event.metadata,
            created_at: event.created_at,
        }
    }
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

#[derive(Debug, Clone)]
pub struct ErrorResponse {
    pub status: StatusCode,
    pub body: ErrorResponseBody,
}

impl ErrorResponse {
    pub fn new(
        status: StatusCode,
        error_id: impl Into<String>,
        error_message: impl Into<String>,
    ) -> Self {
        Self {
            status,
            body: ErrorResponseBody {
                error_id: error_id.into(),
                error_message: error_message.into(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct ErrorResponseBody {
    pub error_id: String,
    pub error_message: String,
}

impl IntoResponse for ErrorResponse {
    fn into_response(self) -> Response {
        let data = match serde_json::to_vec(&self.body) {
            Ok(data) => data,
            Err(err) => {
                return Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(err.to_string())
            }
        };
        Response::builder()
            .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
            .status(self.status)
            .body(data)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use alloy::primitives::utils::parse_ether;
    use alloy::primitives::U256;
    use chrono::TimeZone;
    use hex_literal::hex;
    use poem_openapi::types::{ParseFromJSON, ToJSON};
    use serde_json::value::RawValue;
    use test_case::test_case;

    use super::*;

    const ADDRESS: [u8; 20] = hex!("1ed53d680b8890dae2a63f673a85ffde1fd5c7a2");
    const HASH: [u8; 32] = hex!(
        "46239dbfe5502b9f82c3dff992927d8d9b3168e732b4fd5771288569f5a1813d"
    );

    /// Encodes `value` the way the server writes response bodies.
    fn api_json<T: ToJSON>(value: &T) -> String {
        serde_json::to_string(&value.to_json().unwrap()).unwrap()
    }

    #[test]
    fn send_tx_request_api_json() {
        let request = SendTxRequest::parse_from_json(Some(serde_json::json!({
            "to": "0x1Ed53d680B8890DAe2a63f673a85fFDE1FD5C7a2",
            "value": "1000000000000000000",
            "data": "0xDEADBEEF",
            "gasLimit": "21000",
            "priority": "fast",
            "txId": "tx-1",
            "blobs": [[0, 255]]
        })))
        .unwrap();

        assert_eq!(
            api_json(&request),
            r#"{"blobs":[[0,255]],"data":"0xdeadbeef","gasLimit":"21000","priority":"fast","to":"0x1ed53d680b8890dae2a63f673a85ffde1fd5c7a2","txId":"tx-1","value":"1000000000000000000"}"#
        );
    }

    #[test]
    fn send_tx_request_api_json_defaults() {
        let request = SendTxRequest::parse_from_json(Some(serde_json::json!({
            "to": "0x1ed53d680b8890dae2a63f673a85ffde1fd5c7a2",
            "value": "0",
            "gasLimit": "21000"
        })))
        .unwrap();

        assert_eq!(
            api_json(&request),
            r#"{"blobs":null,"data":null,"gasLimit":"21000","priority":"regular","to":"0x1ed53d680b8890dae2a63f673a85ffde1fd5c7a2","txId":null,"value":"0"}"#
        );
    }

    #[test]
    fn get_tx_response_api_json() {
        let response = GetTxResponse {
            tx_id: "tx-1".to_string(),
            to: Address(ADDRESS.into()),
            data: Some(HexBytes::from(vec![0xde, 0xad, 0xbe, 0xef])),
            value: DecimalU256(U256::from(1_000_000_000_000_000_000u64)),
            gas_limit: DecimalU256(U256::from(21_000u64)),
            nonce: 7,
            tx_hash: Some(H256(HASH.into())),
            status: Some(TxStatus::Mined),
        };

        assert_eq!(
            api_json(&response),
            r#"{"data":"0xdeadbeef","gasLimit":"21000","nonce":7,"status":"mined","to":"0x1ed53d680b8890dae2a63f673a85ffde1fd5c7a2","txHash":"0x46239dbfe5502b9f82c3dff992927d8d9b3168e732b4fd5771288569f5a1813d","txId":"tx-1","value":"1000000000000000000"}"#
        );
    }

    #[test]
    fn get_tx_response_api_json_unsent() {
        let response = GetTxResponse {
            tx_id: "tx-1".to_string(),
            to: Address(ADDRESS.into()),
            data: None,
            value: DecimalU256(U256::from(0u64)),
            gas_limit: DecimalU256(U256::from(21_000u64)),
            nonce: 0,
            tx_hash: None,
            status: None,
        };

        assert_eq!(
            api_json(&response),
            r#"{"data":null,"gasLimit":"21000","nonce":0,"status":null,"to":"0x1ed53d680b8890dae2a63f673a85ffde1fd5c7a2","txHash":null,"txId":"tx-1","value":"0"}"#
        );
    }

    const RELAYER_ID: &str = "1b908a34-5dc1-4d2d-a146-5eb46e975830";

    const FULL_SEND_TX_REQUEST_V2: &str = r#"{
        "to": "0x1Ed53d680B8890DAe2a63f673a85fFDE1FD5C7a2",
        "value": "1000000000000000000",
        "data": "0xdeadbeef",
        "gasLimit": "21000",
        "priority": "fast",
        "txId": "tx-1",
        "metadata": { "transactionType": "SafeDeployment", "amount": 18446744073709551617 }
    }"#;

    fn send_tx_request_v2(request: &str) -> SendTxRequestV2 {
        serde_json::from_str(request).unwrap()
    }

    /// Returns the full request with `field` set to the JSON text `value`,
    /// or without `field` if `value` is `None`
    fn full_send_tx_request_v2_with(
        field: &str,
        value: Option<&str>,
    ) -> String {
        let mut request: BTreeMap<String, Box<RawValue>> =
            serde_json::from_str(FULL_SEND_TX_REQUEST_V2).unwrap();

        match value {
            Some(value) => request.insert(
                field.to_string(),
                RawValue::from_string(value.to_string()).unwrap(),
            ),
            None => request.remove(field),
        };

        serde_json::to_string(&request).unwrap()
    }

    #[test]
    fn send_tx_request_v2_keeps_the_metadata_text() {
        let request = send_tx_request_v2(FULL_SEND_TX_REQUEST_V2);

        assert_eq!(
            request.metadata.unwrap().as_str(),
            r#"{ "transactionType": "SafeDeployment", "amount": 18446744073709551617 }"#
        );
    }

    #[test]
    fn request_hash_is_stable() {
        let request = send_tx_request_v2(FULL_SEND_TX_REQUEST_V2);

        // Keccak-256 of the canonical request:
        // {"relayerId":"1b908a34-5dc1-4d2d-a146-5eb46e975830","to":"1ed53d680b8890dae2a63f673a85ffde1fd5c7a2","value":"1000000000000000000","data":"deadbeef","gasLimit":"21000","priority":"fast","metadata":{ "transactionType": "SafeDeployment", "amount": 18446744073709551617 }}
        assert_eq!(
            hex::encode(request.request_hash(RELAYER_ID)),
            "bfafe162c29d763b0cc9b2ef4528f2905dd425e6320173c2059de53a95a0bfe1"
        );
    }

    #[test_case("txId", None ; "without tx id")]
    #[test_case("txId", Some(r#""tx-2""#) ; "with another tx id")]
    #[test_case(
        "to",
        Some(r#""0x1ed53d680b8890dae2a63f673a85ffde1fd5c7a2""#)
        ; "with a lowercase address"
    )]
    fn request_hash_matches_an_equivalent_request(
        field: &str,
        value: Option<&str>,
    ) {
        let request = send_tx_request_v2(FULL_SEND_TX_REQUEST_V2);
        let equivalent =
            send_tx_request_v2(&full_send_tx_request_v2_with(field, value));

        assert_eq!(
            request.request_hash(RELAYER_ID),
            equivalent.request_hash(RELAYER_ID)
        );
    }

    #[test_case("data", r#""0x""# ; "empty data")]
    #[test_case("priority", r#""regular""# ; "regular priority")]
    fn request_hash_treats_a_default_like_an_absent_field(
        field: &str,
        default: &str,
    ) {
        let absent =
            send_tx_request_v2(&full_send_tx_request_v2_with(field, None));
        let explicit = send_tx_request_v2(&full_send_tx_request_v2_with(
            field,
            Some(default),
        ));

        assert_eq!(
            absent.request_hash(RELAYER_ID),
            explicit.request_hash(RELAYER_ID)
        );
    }

    #[test_case(
        "to",
        Some(r#""0x0000000000000000000000000000000000000001""#)
        ; "to"
    )]
    #[test_case("value", Some(r#""1""#) ; "value")]
    #[test_case("data", Some(r#""0xdeadbeee""#) ; "data")]
    #[test_case("data", None ; "absent data")]
    #[test_case("gasLimit", Some(r#""21001""#) ; "gas limit")]
    #[test_case("priority", Some(r#""slow""#) ; "priority")]
    #[test_case(
        "metadata",
        Some(r#"{ "transactionType": "SafeDeployment", "amount": 18446744073709551616 }"#)
        ; "adjacent big integer in metadata"
    )]
    #[test_case(
        "metadata",
        Some(r#"{ "amount": 18446744073709551617, "transactionType": "SafeDeployment" }"#)
        ; "reordered metadata keys"
    )]
    #[test_case(
        "metadata",
        Some(r#"{"transactionType":"SafeDeployment","amount":18446744073709551617}"#)
        ; "metadata whitespace"
    )]
    #[test_case("metadata", Some("{}") ; "empty metadata")]
    #[test_case("metadata", None ; "absent metadata")]
    fn request_hash_differs_for_another_request(
        field: &str,
        value: Option<&str>,
    ) {
        let request = send_tx_request_v2(FULL_SEND_TX_REQUEST_V2);
        let other =
            send_tx_request_v2(&full_send_tx_request_v2_with(field, value));

        assert_ne!(
            request.request_hash(RELAYER_ID),
            other.request_hash(RELAYER_ID)
        );
    }

    #[test]
    fn request_hash_differs_per_relayer() {
        let request = send_tx_request_v2(FULL_SEND_TX_REQUEST_V2);

        assert_ne!(
            request.request_hash(RELAYER_ID),
            request.request_hash("6b4a8f1e-5c4b-4b8e-9a43-1b1b2f0c2a8e")
        );
    }

    #[test_case("blobs", r#"[[0, 255]]"# ; "blobs")]
    #[test_case("txId", r#""""# ; "empty tx id")]
    #[test_case("txId", &format!(r#""{}""#, "x".repeat(256)) ; "long tx id")]
    #[test_case("txId", r#""tx\u0000""# ; "tx id with nul")]
    #[test_case("metadata", "[1]" ; "metadata array")]
    #[test_case("gasLimit", r#""-1""# ; "negative gas limit")]
    #[test_case("value", "1" ; "value as a number")]
    fn send_tx_request_v2_rejects_invalid_fields(field: &str, value: &str) {
        let request = full_send_tx_request_v2_with(field, Some(value));

        serde_json::from_str::<SendTxRequestV2>(&request).unwrap_err();
    }

    #[test]
    fn send_tx_request_v2_accepts_the_longest_tx_id() {
        let tx_id = "\u{e9}".repeat(255);

        let request = send_tx_request_v2(&full_send_tx_request_v2_with(
            "txId",
            Some(&format!(r#""{tx_id}""#)),
        ));

        assert_eq!(request.tx_id, Some(tx_id));
    }

    #[test]
    fn send_tx_request_v2_treats_null_like_an_absent_field() {
        let request = send_tx_request_v2(
            r#"{ "to": "0x1ed53d680b8890dae2a63f673a85ffde1fd5c7a2", "value": "0", "gasLimit": "21000", "data": null, "txId": null, "metadata": null }"#,
        );

        assert!(request.data.is_none());
        assert!(request.tx_id.is_none());
        assert!(request.metadata.is_none());
    }

    #[test]
    fn get_tx_response_v2_json() {
        let response = GetTxResponseV2 {
            tx_id: "tx-1".to_string(),
            relayer_id: RELAYER_ID.to_string(),
            chain_id: 480,
            to: Address(ADDRESS.into()),
            data: HexBytes::from(vec![0xde, 0xad, 0xbe, 0xef]),
            value: DecimalU256(U256::from(1_000_000_000_000_000_000u64)),
            gas_limit: DecimalU256(U256::from(21_000u64)),
            nonce: 7,
            priority: TransactionPriority::Fast,
            metadata: Some(
                serde_json::from_str(r#"{ "amount": 18446744073709551617 }"#)
                    .unwrap(),
            ),
            created_at: Utc.with_ymd_and_hms(2026, 11, 17, 12, 0, 0).unwrap(),
            status: TxStatusV2::Mined,
            tx_hash: Some(H256(HASH.into())),
            mined_at: Some(
                Utc.with_ymd_and_hms(2026, 11, 17, 12, 0, 4).unwrap(),
            ),
        };

        assert_eq!(
            serde_json::to_string(&response).unwrap(),
            r#"{"txId":"tx-1","relayerId":"1b908a34-5dc1-4d2d-a146-5eb46e975830","chainId":480,"to":"0x1ed53d680b8890dae2a63f673a85ffde1fd5c7a2","data":"0xdeadbeef","value":"1000000000000000000","gasLimit":"21000","nonce":7,"priority":"fast","metadata":{ "amount": 18446744073709551617 },"createdAt":"2026-11-17T12:00:00Z","status":"mined","txHash":"0x46239dbfe5502b9f82c3dff992927d8d9b3168e732b4fd5771288569f5a1813d","minedAt":"2026-11-17T12:00:04Z"}"#
        );
    }

    #[test]
    fn get_tx_response_v2_json_queued() {
        let response = GetTxResponseV2 {
            tx_id: "tx-1".to_string(),
            relayer_id: RELAYER_ID.to_string(),
            chain_id: 480,
            to: Address(ADDRESS.into()),
            data: HexBytes::from(vec![]),
            value: DecimalU256(U256::ZERO),
            gas_limit: DecimalU256(U256::from(21_000u64)),
            nonce: 0,
            priority: TransactionPriority::Regular,
            metadata: None,
            created_at: Utc.with_ymd_and_hms(2026, 11, 17, 12, 0, 0).unwrap(),
            status: TxStatusV2::Queued,
            tx_hash: None,
            mined_at: None,
        };

        assert_eq!(
            serde_json::to_string(&response).unwrap(),
            r#"{"txId":"tx-1","relayerId":"1b908a34-5dc1-4d2d-a146-5eb46e975830","chainId":480,"to":"0x1ed53d680b8890dae2a63f673a85ffde1fd5c7a2","data":"0x","value":"0","gasLimit":"21000","nonce":0,"priority":"regular","metadata":null,"createdAt":"2026-11-17T12:00:00Z","status":"queued","txHash":null,"minedAt":null}"#
        );
    }

    #[test]
    fn relayer_response_api_json() {
        let response = RelayerResponse {
            id: "id".to_string(),
            name: "name".to_string(),
            chain_id: 1,
            key_id: "key_id".to_string(),
            address: Address(ADDRESS.into()),
            nonce: 2,
            current_nonce: 1,
            max_inflight_txs: 5,
            max_queued_txs: 20,
            gas_price_limits: vec![RelayerGasPriceLimitResponse {
                value: DecimalU256(U256::from(100_000_000_000u64)),
                chain_id: 1,
            }],
            enabled: true,
        };

        assert_eq!(
            api_json(&response),
            r#"{"address":"0x1ed53d680b8890dae2a63f673a85ffde1fd5c7a2","chainId":1,"currentNonce":1,"enabled":true,"gasPriceLimits":[{"chainId":1,"value":"100000000000"}],"id":"id","keyId":"key_id","maxInflightTxs":5,"maxQueuedTxs":20,"name":"name","nonce":2}"#
        );
    }

    #[test]
    fn create_relayer_response_api_json() {
        let response = CreateRelayerResponse {
            relayer_id: "id".to_string(),
            address: Address(ADDRESS.into()),
        };

        assert_eq!(
            api_json(&response),
            r#"{"address":"0x1ed53d680b8890dae2a63f673a85ffde1fd5c7a2","relayerId":"id"}"#
        );
    }

    #[test]
    fn relayer_response_serialize() {
        let info = RelayerResponse {
            id: "id".to_string(),
            name: "name".to_string(),
            chain_id: 1,
            key_id: "key_id".to_string(),
            address: Address::default(),
            nonce: 0,
            current_nonce: 0,
            max_inflight_txs: 0,
            max_queued_txs: 0,
            gas_price_limits: vec![RelayerGasPriceLimitResponse {
                value: U256::ZERO.into(),
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
        let value = parse_ether("1").unwrap();

        let request = SendTxRequest {
            to: Address::default(),
            value: value.into(),
            data: Some(HexBytes::from(vec![0])),
            gas_limit: U256::ZERO.into(),
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
