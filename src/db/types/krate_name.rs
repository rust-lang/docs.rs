use anyhow::{Result, bail};
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
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, Deref, Into, Display, DeserializeFromStr, SerializeDisplay,
)]
pub struct KrateName(String);

impl FromStr for KrateName {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        validate_crate_name(s)?;
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

/// validate if a string is a valid crate name.
/// Based on the crates.io implementation in their publish-endpoint:
/// https://github.com/rust-lang/crates.io/blob/9651eaab14887e0442849d5e81c1f2bbf10a73a2/crates/crates_io_database/src/models/krate.rs#L218-L252
fn validate_crate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("empty crate name");
    }
    if name.len() > 64 {
        bail!("crate name too long (maximum is 64 characters)");
    }

    let mut chars = name.chars();
    if let Some(ch) = chars.next() {
        if ch.is_ascii_digit() {
            bail!("crate name cannot start with a digit");
        }
        if !ch.is_ascii_alphabetic() {
            bail!("crate name must start with an alphabetic character");
        }
    }

    for ch in chars {
        if !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_') {
            bail!("invalid character '{}' in crate name", ch);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::test::TestEnvironment;

    use super::*;
    use test_case::test_case;

    #[test_case("valid_crate_name")]
    #[test_case("with-dash")]
    #[test_case("CapitalLetter")]
    fn test_valid_crate_name(name: &str) {
        assert!(validate_crate_name(name).is_ok());
        assert_eq!(name.parse::<KrateName>().unwrap(), name);
    }

    #[test_case("with space")]
    #[test_case("line break\n")]
    #[test_case("non ascii äöü")]
    #[test_case("0123456789101112131415161718192021222324252627282930313233343536373839"; "too long")]
    fn test_invalid_crate_name(name: &str) {
        assert!(validate_crate_name(name).is_err());
        assert!(name.parse::<KrateName>().is_err());
    }

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
