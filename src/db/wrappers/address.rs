use std::fmt;

use ethers::types::Address;
use serde::{Deserialize, Serialize};
use sqlx::database::HasValueRef;
use sqlx::Database;

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AddressWrapper(pub Address);

impl fmt::Display for AddressWrapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}", self.0)
    }
}

impl<'r, DB> sqlx::Decode<'r, DB> for AddressWrapper
where
    DB: Database,
    Vec<u8>: sqlx::Decode<'r, DB>,
{
    fn decode(
        value: <DB as HasValueRef<'r>>::ValueRef,
    ) -> Result<Self, sqlx::error::BoxDynError> {
        let bytes = <Vec<u8> as sqlx::Decode<DB>>::decode(value)?;

        let address = Address::from_slice(&bytes);

        Ok(Self(address))
    }
}

impl<DB: Database> sqlx::Type<DB> for AddressWrapper
where
    Vec<u8>: sqlx::Type<DB>,
{
    fn type_info() -> DB::TypeInfo {
        <Vec<u8> as sqlx::Type<DB>>::type_info()
    }

    fn compatible(ty: &DB::TypeInfo) -> bool {
        *ty == Self::type_info()
    }
}

impl From<Address> for AddressWrapper {
    fn from(value: Address) -> Self {
        Self(value)
    }
}

impl From<base_api_types::Address> for AddressWrapper {
    fn from(value: base_api_types::Address) -> Self {
        Self(value.0)
    }
}

impl From<AddressWrapper> for base_api_types::Address {
    fn from(value: AddressWrapper) -> Self {
        Self(value.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_outputs_full_address() {
        let address = "0x0123456789abcdef0123456789abcdef01234567"
            .parse()
            .unwrap();

        assert_eq!(
            AddressWrapper(address).to_string(),
            "0x0123456789abcdef0123456789abcdef01234567"
        );
    }
}
