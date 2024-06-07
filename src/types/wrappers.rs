use decimal_u256::DecimalU256;
use hex_u256::HexU256;

pub mod address;
pub mod h256;
pub mod hex_bytes;

// TODO: Remove repeated code in these 2 modules
pub mod decimal_u256;
pub mod hex_u256;

impl From<HexU256> for DecimalU256 {
    fn from(value: HexU256) -> DecimalU256 {
        DecimalU256::from(value.0)
    }
}

impl From<DecimalU256> for HexU256 {
    fn from(value: DecimalU256) -> HexU256 {
        HexU256::from(value.0)
    }
}
