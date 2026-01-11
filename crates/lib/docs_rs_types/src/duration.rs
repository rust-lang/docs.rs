mod duration_impl {
    use anyhow::Result;
    use derive_more::{Deref, From, Into};
    use sqlx::postgres::types::PgInterval;
    use sqlx::{
        Postgres,
        error::BoxDynError,
        postgres::{PgTypeInfo, PgValueRef},
        prelude::*,
    };
    use std::time::Duration as StdDuration;

    /// NewType around std Duration to be able to use it with sqlx.
    ///
    /// For now only for decoding intervals from the database.
    #[derive(Clone, Debug, Deref, Eq, From, Hash, Into, PartialEq)]
    pub struct Duration(pub StdDuration);

    impl Duration {
        pub const fn from_secs(secs: u64) -> Duration {
            Self(StdDuration::from_secs(secs))
        }
    }

    impl Type<Postgres> for Duration {
        fn type_info() -> PgTypeInfo {
            <PgInterval as Type<Postgres>>::type_info()
        }

        fn compatible(ty: &PgTypeInfo) -> bool {
            <PgInterval as Type<Postgres>>::compatible(ty)
        }
    }

    impl TryFrom<PgInterval> for Duration {
        type Error = crate::convert::IntervalError;

        fn try_from(value: PgInterval) -> Result<Self, Self::Error> {
            Ok(Self(crate::convert::interval_to_duration(value)?))
        }
    }

    impl<'r> Decode<'r, Postgres> for Duration {
        fn decode(value: PgValueRef<'r>) -> Result<Self, BoxDynError> {
            let interval: PgInterval = Decode::<Postgres>::decode(value)?;

            Ok(interval.try_into()?)
        }
    }
}

pub use duration_impl::Duration;
