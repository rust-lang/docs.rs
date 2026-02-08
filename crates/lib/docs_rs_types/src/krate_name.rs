use anyhow::Result;
use crates_io_validation::{InvalidCrateName, validate_crate_name};
use serde_with::{DeserializeFromStr, SerializeDisplay};
use sqlx::{
    Decode, Encode, Postgres,
    encode::IsNull,
    error::BoxDynError,
    postgres::{PgArgumentBuffer, PgHasArrayType, PgTypeInfo, PgValueRef},
    prelude::*,
};
use std::{borrow::Cow, fmt, io::Write, str::FromStr};

/// validated crate name
///
/// Right now only used in web::cache, but we'll probably also use it
/// to match our routes later.
///
#[derive(Debug, Clone, Eq, PartialOrd, Ord, Hash, DeserializeFromStr, SerializeDisplay)]
pub struct KrateName(Cow<'static, str>);

impl KrateName {
    #[cfg(any(test, feature = "testing"))]
    pub const fn from_static(s: &'static str) -> Self {
        KrateName(Cow::Borrowed(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl AsRef<str> for KrateName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<KrateName> for Cow<'static, str> {
    fn from(krate_name: KrateName) -> Self {
        krate_name.0
    }
}

impl fmt::Display for KrateName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&KrateName> for KrateName {
    fn from(krate_name: &KrateName) -> Self {
        krate_name.clone()
    }
}

impl FromStr for KrateName {
    type Err = InvalidCrateName;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        validate_crate_name("crate", s)?;
        Ok(KrateName(Cow::Owned(s.to_string())))
    }
}

impl TryFrom<&str> for KrateName {
    type Error = InvalidCrateName;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl Type<Postgres> for KrateName {
    fn type_info() -> PgTypeInfo {
        <String as Type<Postgres>>::type_info()
    }

    fn compatible(ty: &PgTypeInfo) -> bool {
        <String as Type<Postgres>>::compatible(ty)
    }
}

impl<'q> Encode<'q, Postgres> for KrateName {
    fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
        write!(**buf, "{}", self.0)?;
        Ok(IsNull::No)
    }
}

impl<'r> Decode<'r, Postgres> for KrateName {
    fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
        // future improvement: we could also avoid the allocation here
        // and return a Cow::Borrowed while reading from the DB.
        // But this would mean the lifetime leaks into the whole codebase.
        let s: &str = Decode::<Postgres>::decode(value)?;
        Ok(Self(Cow::Owned(s.parse()?)))
    }
}

impl PgHasArrayType for KrateName {
    fn array_type_info() -> PgTypeInfo {
        <&str as PgHasArrayType>::array_type_info()
    }

    fn array_compatible(ty: &PgTypeInfo) -> bool {
        <&str as PgHasArrayType>::array_compatible(ty)
    }
}

impl<T> PartialEq<T> for KrateName
where
    T: AsRef<str>,
{
    fn eq(&self, other: &T) -> bool {
        self.0 == other.as_ref()
    }
}

#[cfg(test)]
mod tests {
    // use super::*;
    // use crate::test::TestEnvironment;

    // TODO: disabling test temporarily, other things will fail if this would fail

    // #[tokio::test(flavor = "multi_thread")]
    // async fn test_sqlx_encode_decode() -> Result<()> {
    //     let env = TestEnvironment::new().await?;
    //     let mut conn = env.async_db().async_conn().await;

    //     let some_crate_name = "some-krate-123".parse::<KrateName>()?;

    //     sqlx::query!(
    //         "INSERT INTO crates (name) VALUES ($1)",
    //         some_crate_name as _
    //     )
    //     .execute(&mut *conn)
    //     .await?;

    //     let new_name = sqlx::query_scalar!(r#"SELECT name as "name: KrateName" FROM crates"#)
    //         .fetch_one(&mut *conn)
    //         .await?;

    //     assert_eq!(new_name, some_crate_name);

    //     Ok(())
    // }
}
