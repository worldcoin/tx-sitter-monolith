use decimal_u256::DecimalU256Wrapper;
use hex_u256::HexU256Wrapper;

pub mod address;
pub mod h256;
pub mod hex_bytes;

// TODO: Remove repeated code in these 2 modules
pub mod decimal_u256;
pub mod hex_u256;

impl From<HexU256Wrapper> for DecimalU256Wrapper {
    fn from(value: HexU256Wrapper) -> DecimalU256Wrapper {
        DecimalU256Wrapper::from(value.0)
    }
}

impl From<DecimalU256Wrapper> for HexU256Wrapper {
    fn from(value: DecimalU256Wrapper) -> HexU256Wrapper {
        HexU256Wrapper::from(value.0)
    }
}
