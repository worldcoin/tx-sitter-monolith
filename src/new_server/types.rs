use ethers::types::Address;
use poem_openapi::Object;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddressWrapper(pub Address);

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct CreateRelayerRequest {
    /// New relayer name
    pub name: String,
    pub chain_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Object)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub struct CreateRelayerResponse {
    pub relayer_id: String,
    pub address: AddressWrapper,
}
