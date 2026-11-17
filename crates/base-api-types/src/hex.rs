use std::fmt;

/// Error returned when a string is not hex, optionally `0x` prefixed.
#[derive(Debug)]
pub struct InvalidHex;

impl fmt::Display for InvalidHex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected hex digits with an optional 0x prefix")
    }
}

/// Strips the optional `0x` prefix from `s` and returns its hex digits.
///
/// Only a lowercase prefix is accepted. The alloy parsers would also accept
/// `0X`, so callers parse the returned digits instead of the raw input.
pub fn hex_digits(s: &str) -> Result<&str, InvalidHex> {
    let digits = s.strip_prefix("0x").unwrap_or(s);

    if digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(digits)
    } else {
        Err(InvalidHex)
    }
}
