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
