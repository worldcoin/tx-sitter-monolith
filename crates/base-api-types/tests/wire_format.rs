//! Golden tests for the JSON wire format of the API primitives.
//!
//! The server encodes and decodes these types through poem-openapi, while
//! `tx-sitter-client` goes through serde, so every case runs both paths and
//! requires them to agree.

use std::fmt::Debug;

use alloy_primitives::U256;
use base_api_types::{Address, DecimalU256, HexBytes, HexU256, H256};
use hex_literal::hex;
use poem_openapi::types::{ParseFromJSON, ToJSON};
use serde::de::DeserializeOwned;
use serde::Serialize;
use test_case::test_case;

const ADDRESS: [u8; 20] = hex!("1ed53d680b8890dae2a63f673a85ffde1fd5c7a2");
const HASH: [u8; 32] =
    hex!("46239dbfe5502b9f82c3dff992927d8d9b3168e732b4fd5771288569f5a1813d");
const U256_MAX_DECIMAL: &str = "115792089237316195423570985008687907853269984665640564039457584007913129639935";

fn encode<T: Serialize + ToJSON>(value: &T) -> String {
    let via_serde = serde_json::to_string(value).unwrap();
    let via_poem = serde_json::to_string(&value.to_json().unwrap()).unwrap();

    assert_eq!(via_serde, via_poem, "serde and poem encodings differ");

    via_serde
}

fn decode<T>(json: &str) -> Option<T>
where
    T: DeserializeOwned + ParseFromJSON + PartialEq + Debug,
{
    let via_serde = serde_json::from_str::<T>(json).ok();
    let via_poem =
        T::parse_from_json(Some(serde_json::from_str(json).unwrap())).ok();

    assert_eq!(
        via_serde, via_poem,
        "serde and poem decodings differ: {json}"
    );

    via_serde
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

#[test]
fn address_encodes_as_lowercase_hex() {
    assert_eq!(
        encode(&Address(ADDRESS.into())),
        json_string("0x1ed53d680b8890dae2a63f673a85ffde1fd5c7a2")
    );
    assert_eq!(
        encode(&Address::default()),
        json_string("0x0000000000000000000000000000000000000000")
    );
}

#[test_case("0x1ed53d680b8890dae2a63f673a85ffde1fd5c7a2" ; "lowercase")]
#[test_case("0x1Ed53d680B8890DAe2a63f673a85fFDE1FD5C7a2" ; "checksummed")]
#[test_case("0x1ed53d680B8890DAe2a63f673a85fFDE1FD5C7a2" ; "wrong checksum")]
#[test_case("0x1ED53D680B8890DAE2A63F673A85FFDE1FD5C7A2" ; "uppercase")]
#[test_case("1Ed53d680B8890DAe2a63f673a85fFDE1FD5C7a2" ; "without prefix")]
fn address_decodes(s: &str) {
    assert_eq!(decode(&json_string(s)), Some(Address(ADDRESS.into())));
}

#[test_case(&json_string("0x1ed53d680b8890dae2a63f673a85ffde1fd5c7") ; "too short")]
#[test_case(&json_string("0x1ed53d680b8890dae2a63f673a85ffde1fd5c7a200") ; "too long")]
#[test_case(&json_string("0x1ed53d680b8890dae2a63f673a85ffde1fd5c7ag") ; "non hex")]
#[test_case(&json_string("0X1ed53d680b8890dae2a63f673a85ffde1fd5c7a2") ; "uppercase prefix")]
#[test_case(&json_string("") ; "empty")]
#[test_case(&serde_json::to_string(&ADDRESS).unwrap() ; "byte array")]
#[test_case("1" ; "number")]
#[test_case("null" ; "null")]
fn address_rejects(json: &str) {
    assert_eq!(decode::<Address>(json), None);
}

#[test]
fn h256_encodes_as_lowercase_hex() {
    assert_eq!(
        encode(&H256(HASH.into())),
        json_string(
            "0x46239dbfe5502b9f82c3dff992927d8d9b3168e732b4fd5771288569f5a1813d"
        )
    );
}

#[test_case("0x46239dbfe5502b9f82c3dff992927d8d9b3168e732b4fd5771288569f5a1813d" ; "lowercase")]
#[test_case("0x46239DBFE5502B9F82C3DFF992927D8D9B3168E732B4FD5771288569F5A1813D" ; "uppercase")]
#[test_case("46239dbfe5502b9f82c3dff992927d8d9b3168e732b4fd5771288569f5a1813d" ; "without prefix")]
fn h256_decodes(s: &str) {
    assert_eq!(decode(&json_string(s)), Some(H256(HASH.into())));
}

#[test_case(&json_string("0x46239dbfe5502b9f82c3dff992927d8d9b3168e732b4fd5771288569f5a181") ; "too short")]
#[test_case(&json_string("0x46239dbfe5502b9f82c3dff992927d8d9b3168e732b4fd5771288569f5a1813d00") ; "too long")]
#[test_case(&json_string("0x46239dbfe5502b9f82c3dff992927d8d9b3168e732b4fd5771288569f5a1813g") ; "non hex")]
#[test_case(&json_string("") ; "empty")]
#[test_case(&serde_json::to_string(&HASH).unwrap() ; "byte array")]
#[test_case("1" ; "number")]
fn h256_rejects(json: &str) {
    assert_eq!(decode::<H256>(json), None);
}

#[test_case(vec![], "0x" ; "empty")]
#[test_case(vec![0], "0x00" ; "zero byte")]
#[test_case(vec![0xde, 0xad, 0xbe, 0xef], "0xdeadbeef" ; "lowercase")]
fn hex_bytes_encodes(bytes: Vec<u8>, expected: &str) {
    assert_eq!(encode(&HexBytes::from(bytes)), json_string(expected));
}

#[test_case("0x", vec![] ; "empty with prefix")]
#[test_case("", vec![] ; "empty without prefix")]
#[test_case("0x00", vec![0] ; "zero byte")]
#[test_case("0xDEADBEEF", vec![0xde, 0xad, 0xbe, 0xef] ; "uppercase")]
#[test_case("deadbeef", vec![0xde, 0xad, 0xbe, 0xef] ; "without prefix")]
fn hex_bytes_decodes(s: &str, expected: Vec<u8>) {
    assert_eq!(decode(&json_string(s)), Some(HexBytes::from(expected)));
}

#[test_case(&json_string("0xabc") ; "odd length")]
#[test_case(&json_string("0xzz") ; "non hex")]
#[test_case("[1,2]" ; "byte array")]
#[test_case("1" ; "number")]
#[test_case("null" ; "null")]
fn hex_bytes_rejects(json: &str) {
    assert_eq!(decode::<HexBytes>(json), None);
}

#[test_case(U256::from(0u64), "0x0" ; "zero")]
#[test_case(U256::from(255u64), "0xff" ; "one byte")]
#[test_case(U256::from(256u64), "0x100" ; "odd number of digits")]
#[test_case(U256::MAX, "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff" ; "max")]
fn hex_u256_encodes_without_leading_zeros(value: U256, expected: &str) {
    assert_eq!(encode(&HexU256(value)), json_string(expected));
}

#[test_case("0x0", U256::from(0u64) ; "zero")]
#[test_case("0xff", U256::from(255u64) ; "lowercase")]
#[test_case("0xFF", U256::from(255u64) ; "uppercase")]
#[test_case("0x00ff", U256::from(255u64) ; "leading zeros")]
#[test_case("10", U256::from(16u64) ; "without prefix is still hex")]
#[test_case("0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff", U256::MAX ; "max")]
fn hex_u256_decodes(s: &str, expected: U256) {
    assert_eq!(decode(&json_string(s)), Some(HexU256(expected)));
}

#[test_case(&json_string("0x") ; "no digits")]
#[test_case(&json_string("0x10000000000000000000000000000000000000000000000000000000000000000") ; "overflow")]
#[test_case(&json_string("0x00000000000000000000000000000000000000000000000000000000000000001") ; "more than 64 digits")]
#[test_case(&json_string("0xzz") ; "non hex")]
#[test_case(&json_string("0x1_0") ; "underscore")]
#[test_case("1" ; "number")]
fn hex_u256_rejects(json: &str) {
    assert_eq!(decode::<HexU256>(json), None);
}

#[test_case(U256::from(0u64), "0" ; "zero")]
#[test_case(U256::from(1_000_000_000_000_000_000u64), "1000000000000000000" ; "one ether")]
#[test_case(U256::MAX, U256_MAX_DECIMAL ; "max")]
fn decimal_u256_encodes(value: U256, expected: &str) {
    assert_eq!(encode(&DecimalU256(value)), json_string(expected));
}

#[test_case("0", U256::from(0u64) ; "zero")]
#[test_case("21000", U256::from(21_000u64) ; "gas limit")]
#[test_case("007", U256::from(7u64) ; "leading zeros")]
#[test_case("", U256::from(0u64) ; "empty string is zero")]
#[test_case(U256_MAX_DECIMAL, U256::MAX ; "max")]
fn decimal_u256_decodes(s: &str, expected: U256) {
    assert_eq!(decode(&json_string(s)), Some(DecimalU256(expected)));
}

#[test_case(&json_string("115792089237316195423570985008687907853269984665640564039457584007913129639936") ; "overflow")]
#[test_case(&json_string("-1") ; "negative")]
#[test_case(&json_string("+1") ; "plus sign")]
#[test_case(&json_string("1.5") ; "fraction")]
#[test_case(&json_string("1e18") ; "exponent")]
#[test_case(&json_string("0x10") ; "hex")]
#[test_case(&json_string(" 1") ; "whitespace")]
#[test_case(&json_string("1_000") ; "underscore")]
#[test_case("1" ; "number")]
#[test_case("null" ; "null")]
fn decimal_u256_rejects(json: &str) {
    assert_eq!(decode::<DecimalU256>(json), None);
}
