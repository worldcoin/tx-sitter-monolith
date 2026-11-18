use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::postgres::{PgArgumentBuffer, PgTypeInfo, PgValueFormat, PgValueRef};
use sqlx::{Decode, Encode, Postgres};

/// A Postgres `xid8`: a transaction id that, unlike `xid`, never wraps around
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Xid8(pub u64);

impl sqlx::Type<Postgres> for Xid8 {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_name("xid8")
    }
}

impl Encode<'_, Postgres> for Xid8 {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> IsNull {
        buf.extend(&self.0.to_be_bytes());

        IsNull::No
    }
}

impl Decode<'_, Postgres> for Xid8 {
    fn decode(value: PgValueRef<'_>) -> Result<Self, BoxDynError> {
        let xid = match value.format() {
            PgValueFormat::Binary => {
                u64::from_be_bytes(value.as_bytes()?.try_into()?)
            }
            PgValueFormat::Text => value.as_str()?.parse()?,
        };

        Ok(Self(xid))
    }
}
