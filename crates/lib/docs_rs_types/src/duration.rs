mod duration_impl {
    use derive_more::{Add, AddAssign, Sum};
    use sqlx::postgres::types::PgInterval;
    use sqlx::{
        Postgres,
        error::BoxDynError,
        postgres::{PgTypeInfo, PgValueRef},
        prelude::*,
    };
    use std::{fmt, ops::Deref, str::FromStr, time::Duration as StdDuration};

    /// NewType around std Duration to be able to use it with sqlx.
    ///
    /// For now only for decoding intervals from the database.
    #[derive(Clone, Debug, Eq, Hash, PartialEq, PartialOrd, Sum, Ord, Copy, Add, AddAssign)]
    pub struct Duration(pub StdDuration);

    // Forward constructors returning StdDuration, wrapping their results in Self.
    macro_rules! duration_constructors {
        ($($name:ident($($arg:ident: $ty:ty),* $(,)?);)*) => {
            $(
                pub const fn $name($($arg: $ty),*) -> Self {
                    Self(StdDuration::$name($($arg),*))
                }
            )*
        };
    }

    impl Duration {
        pub const ZERO: Self = Self::from_nanos(0);

        duration_constructors! {
            from_nanos(nanos: u64);
            from_secs(secs: u64);
            from_mins(mins: u64);
            from_hours(hours: u64);
        }

        pub const fn from_days(days: u64) -> Duration {
            // nightly only API, we already add it because it's nice.
            Self::from_hours(days * 24)
        }

        /// Round to the nearest millisecond, rounding half milliseconds up.
        /// Saturates at the largest representable whole-millisecond duration.
        pub fn round_to_millis(self) -> Self {
            let nanos = self.0.subsec_nanos() % 1_000_000;
            let truncated = self.0 - StdDuration::from_nanos(u64::from(nanos));
            Self(if nanos >= 500_000 {
                truncated
                    .checked_add(StdDuration::from_millis(1))
                    .unwrap_or(truncated)
            } else {
                truncated
            })
        }
    }

    impl Deref for Duration {
        type Target = StdDuration;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl From<Duration> for StdDuration {
        fn from(duration: Duration) -> Self {
            duration.0
        }
    }

    impl From<StdDuration> for Duration {
        fn from(duration: StdDuration) -> Self {
            Self(duration)
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

    impl FromStr for Duration {
        type Err = humantime::DurationError;

        fn from_str(s: &str) -> Result<Duration, Self::Err> {
            if let Ok(secs) = s.parse::<u64>() {
                Ok(Duration::from_secs(secs))
            } else {
                humantime::parse_duration(s).map(Duration)
            }
        }
    }

    impl fmt::Display for Duration {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            humantime::format_duration(self.0).fmt(f)
        }
    }
}

pub use duration_impl::Duration;

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case("1234")]
    #[test_case("1234s")]
    fn test_parse_secs_with_or_without_unit(input: &str) {
        let duration: Duration = input.parse().unwrap();
        assert_eq!(duration, Duration::from_secs(1234));
    }

    #[test]
    fn rounds_to_milliseconds() {
        for (nanos, expected) in [
            (499_999, 0),
            (500_000, 1_000_000),
            (246_987_074, 247_000_000),
            (999_500_000, 1_000_000_000),
        ] {
            assert_eq!(
                Duration::from_nanos(nanos).round_to_millis(),
                Duration::from_nanos(expected)
            );
        }
    }

    #[test]
    fn test_parse_min() {
        let duration: Duration = "4m".parse().unwrap();
        assert_eq!(duration, Duration::from_mins(4));
    }
}
