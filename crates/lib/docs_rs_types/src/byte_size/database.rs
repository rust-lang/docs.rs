use super::ByteSize;
use sqlx::{
    Postgres,
    encode::IsNull,
    error::BoxDynError,
    postgres::{PgArgumentBuffer, PgTypeInfo, PgValueRef},
    prelude::*,
};
use std::num;

impl Type<Postgres> for ByteSize {
    fn type_info() -> PgTypeInfo {
        <i64 as Type<Postgres>>::type_info()
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        <i64 as Type<Postgres>>::compatible(ty)
    }
}

impl TryFrom<i64> for ByteSize {
    type Error = num::TryFromIntError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Ok(Self(value.try_into()?))
    }
}

impl<'q> Encode<'q, Postgres> for ByteSize {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
        let bytes = i64::try_from(self.0)?;
        <i64 as Encode<Postgres>>::encode_by_ref(&bytes, buf)
    }
}

impl<'r> Decode<'r, Postgres> for ByteSize {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        let bytes = <i64 as Decode<Postgres>>::decode(value)?;
        Ok(bytes.try_into()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_bigint_type() {
        assert_eq!(ByteSize::type_info(), <i64 as Type<Postgres>>::type_info());
        assert!(ByteSize::compatible(&PgTypeInfo::with_name("INT8")));
        assert!(!ByteSize::compatible(&PgTypeInfo::with_name("INTERVAL")));
    }

    #[test]
    fn converts_nonnegative_bigints() {
        for bytes in [0, 1, i64::MAX] {
            assert_eq!(ByteSize::try_from(bytes).unwrap().0, bytes as u64);
        }
        assert!(ByteSize::try_from(-1_i64).is_err());
        assert!(ByteSize::try_from(i64::MIN).is_err());
    }

    #[test]
    fn encodes_bigints_without_wrapping() {
        for bytes in [0, 1, i64::MAX as u64] {
            let mut buf = PgArgumentBuffer::default();
            let result = ByteSize(bytes).encode_by_ref(&mut buf).unwrap();
            assert!(matches!(result, IsNull::No));
            assert_eq!(&buf[..], &(bytes as i64).to_be_bytes());
        }
        for bytes in [i64::MAX as u64 + 1, u64::MAX] {
            let mut buf = PgArgumentBuffer::default();
            assert!(ByteSize(bytes).encode_by_ref(&mut buf).is_err());
            assert!(buf.is_empty());
        }
    }
}
