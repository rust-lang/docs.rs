use anyhow::Result;
use crates_io_validation::{InvalidCrateName, validate_crate_name};
use derive_more::{Deref, Display, Into};
use serde_with::{DeserializeFromStr, SerializeDisplay};
use sqlx::{
    Decode, Encode, Postgres,
    encode::IsNull,
    error::BoxDynError,
    postgres::{PgArgumentBuffer, PgTypeInfo, PgValueRef},
    prelude::*,
};
use std::{io::Write, str::FromStr};

/// validated crate name
///
/// Right now only used in web::cache, but we'll probably also use it
/// to match our routes later.
///
/// FIXME: this should actually come from some shared crate between the rust projects,
/// so the amount of duplication is less.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Deref,
    Into,
    Display,
    DeserializeFromStr,
    SerializeDisplay,
    bincode::Encode,
)]
pub struct KrateName(String);

impl From<&KrateName> for KrateName {
    fn from(krate_name: &KrateName) -> Self {
        krate_name.clone()
    }
}

impl FromStr for KrateName {
    type Err = InvalidCrateName;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        validate_crate_name("crate", s)?;
        Ok(KrateName(s.to_string()))
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
        let s: &str = Decode::<Postgres>::decode(value)?;
        Ok(Self(s.parse()?))
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
    use super::*;
    use crate::test::TestEnvironment;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_sqlx_encode_decode() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let mut conn = env.async_db().async_conn().await;

        let some_crate_name = "some-krate-123".parse::<KrateName>()?;

        sqlx::query!(
            "INSERT INTO crates (name) VALUES ($1)",
            some_crate_name as _
        )
        .execute(&mut *conn)
        .await?;

        let new_name = sqlx::query_scalar!(r#"SELECT name as "name: KrateName" FROM crates"#)
            .fetch_one(&mut *conn)
            .await?;

        assert_eq!(new_name, some_crate_name);

        Ok(())
    }
}
