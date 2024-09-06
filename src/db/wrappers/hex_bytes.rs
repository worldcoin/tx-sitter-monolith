use base_api_types::HexBytes;
use ethers::types::Bytes;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HexBytesWrapper(pub Bytes);

impl From<HexBytes> for HexBytesWrapper {
    fn from(value: HexBytes) -> Self {
        Self(value.0)
    }
}

impl From<HexBytesWrapper> for HexBytes {
    fn from(value: HexBytesWrapper) -> Self {
        Self(value.0)
    }
}

impl From<Bytes> for HexBytesWrapper {
    fn from(value: Bytes) -> Self {
        Self(value)
    }
}

impl From<Vec<u8>> for HexBytesWrapper {
    fn from(value: Vec<u8>) -> Self {
        Self(Bytes::from(value))
    }
}
