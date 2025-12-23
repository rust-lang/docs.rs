pub use semver::VersionReq;

#[allow(clippy::disallowed_types)]
mod version_impl {
    use anyhow::Result;
    use derive_more::{Deref, Display, From, Into};
    use serde_with::{DeserializeFromStr, SerializeDisplay};
    use sqlx::{
        Postgres,
        encode::IsNull,
        error::BoxDynError,
        postgres::{PgArgumentBuffer, PgTypeInfo, PgValueRef},
        prelude::*,
    };
    use std::{io::Write, str::FromStr};

    /// NewType around semver::Version to be able to use it with sqlx.
    ///
    /// Represented as string in the database.
    #[derive(
        Clone,
        Debug,
        Deref,
        DeserializeFromStr,
        Display,
        Eq,
        From,
        Hash,
        Into,
        PartialEq,
        SerializeDisplay,
    )]
    pub struct Version(pub semver::Version);

    impl Version {
        pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
            Self(semver::Version::new(major, minor, patch))
        }

        pub fn parse(text: &str) -> Result<Self, semver::Error> {
            Version::from_str(text)
        }
    }

    impl Type<Postgres> for Version {
        fn type_info() -> PgTypeInfo {
            <String as Type<Postgres>>::type_info()
        }

        fn compatible(ty: &PgTypeInfo) -> bool {
            <String as Type<Postgres>>::compatible(ty)
        }
    }

    impl<'q> Encode<'q, Postgres> for Version {
        fn encode_by_ref(&self, buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
            write!(**buf, "{}", self.0)?;
            Ok(IsNull::No)
        }
    }

    impl<'r> Decode<'r, Postgres> for Version {
        fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
            let s: &str = Decode::<Postgres>::decode(value)?;
            Ok(Self(s.parse()?))
        }
    }

    impl FromStr for Version {
        type Err = semver::Error;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            Ok(Version(semver::Version::from_str(s)?))
        }
    }

    impl TryFrom<&str> for Version {
        type Error = semver::Error;

        fn try_from(value: &str) -> Result<Self, Self::Error> {
            Ok(Version(semver::Version::from_str(value)?))
        }
    }

    impl TryFrom<&String> for Version {
        type Error = semver::Error;

        fn try_from(value: &String) -> Result<Self, Self::Error> {
            Ok(Version(semver::Version::from_str(value)?))
        }
    }

    impl TryFrom<String> for Version {
        type Error = semver::Error;

        fn try_from(value: String) -> Result<Self, Self::Error> {
            Ok(Version(semver::Version::from_str(&value)?))
        }
    }
}

pub use version_impl::Version;
