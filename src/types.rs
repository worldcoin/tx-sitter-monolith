use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;
use sqlx::types::Json;

use crate::db::data::{AddressWrapper, U256Wrapper};

#[derive(Deserialize, Serialize, Debug, Clone, Copy, Default, sqlx::Type)]
#[serde(rename_all = "camelCase")]
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

#[derive(Deserialize, Serialize, Debug, Clone, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct RelayerInfo {
    pub id: String,
    pub name: String,
    #[sqlx(try_from = "i64")]
    pub chain_id: u64,
    pub key_id: String,
    pub address: AddressWrapper,
    #[sqlx(try_from = "i64")]
    pub nonce: u64,
    #[sqlx(try_from = "i64")]
    pub current_nonce: u64,
    #[sqlx(try_from = "i64")]
    pub max_inflight_txs: u64,
    #[sqlx(try_from = "i64")]
    pub max_queued_txs: u64,
    pub gas_price_limits: Json<Vec<RelayerGasPriceLimit>>,
    pub enabled: bool,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct RelayerUpdate {
    #[serde(default)]
    pub relayer_name: Option<String>,
    #[serde(default)]
    pub max_inflight_txs: Option<u64>,
    #[serde(default)]
    pub max_queued_txs: Option<u64>,
    #[serde(default)]
    pub gas_price_limits: Option<Vec<RelayerGasPriceLimit>>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RelayerGasPriceLimit {
    pub value: U256Wrapper,
    pub chain_id: i64,
}

impl RelayerUpdate {
    pub fn with_relayer_name(mut self, relayer_name: String) -> Self {
        self.relayer_name = Some(relayer_name);
        self
    }

    pub fn with_max_inflight_txs(mut self, max_inflight_txs: u64) -> Self {
        self.max_inflight_txs = Some(max_inflight_txs);
        self
    }

    pub fn with_max_queued_txs(mut self, max_queued_txs: u64) -> Self {
        self.max_queued_txs = Some(max_queued_txs);
        self
    }

    pub fn with_gas_price_limits(
        mut self,
        gas_price_limits: Vec<RelayerGasPriceLimit>,
    ) -> Self {
        self.gas_price_limits = Some(gas_price_limits);
        self
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = Some(enabled);
        self
    }
}

#[cfg(test)]
mod tests {
    use ethers::types::{Address, U256};

    use super::*;

    #[test]
    fn relayer_info_serialize() {
        let info = RelayerInfo {
            id: "id".to_string(),
            name: "name".to_string(),
            chain_id: 1,
            key_id: "key_id".to_string(),
            address: AddressWrapper(Address::zero()),
            nonce: 0,
            current_nonce: 0,
            max_inflight_txs: 0,
            max_queued_txs: 0,
            gas_price_limits: Json(vec![RelayerGasPriceLimit {
                value: U256Wrapper(U256::zero()),
                chain_id: 1,
            }]),
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
                  "value": "0x0",
                  "chainId": 1
                }
              ],
              "enabled": true
            }
        "#};

        assert_eq!(json.trim(), expected.trim());
    }
}
