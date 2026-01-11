use sqlx::postgres::types::PgInterval;
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum IntervalError {
    #[error("months not supported")]
    MonthsNotSupported,
    #[error("negative duration")]
    NegativeDuration,
    #[error("duration too large")]
    DurationTooLarge,
}

pub(crate) fn interval_to_duration(interval: PgInterval) -> Result<Duration, IntervalError> {
    if interval.months != 0 {
        return Err(IntervalError::MonthsNotSupported);
    }

    if interval.days < 0 || interval.microseconds < 0 {
        return Err(IntervalError::NegativeDuration);
    }

    Ok(Duration::from_hours(interval.days as u64 * 24)
        + Duration::from_micros(interval.microseconds as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_month_is_invalid() {
        let interval = PgInterval {
            months: 1,
            days: 0,
            microseconds: 0,
        };
        let result = interval_to_duration(interval);
        assert!(matches!(result, Err(IntervalError::MonthsNotSupported)));
    }

    #[test]
    fn test_negative_day_is_invalid() {
        let interval = PgInterval {
            months: 0,
            days: -1,
            microseconds: 0,
        };
        let result = interval_to_duration(interval);
        assert!(matches!(result, Err(IntervalError::NegativeDuration)));
    }

    #[test]
    fn test_negative_ms_is_invalid() {
        let interval = PgInterval {
            months: 0,
            days: 0,
            microseconds: -1,
        };
        let result = interval_to_duration(interval);
        assert!(matches!(result, Err(IntervalError::NegativeDuration)));
    }

    #[test]
    fn test_simple_conversion() {
        let interval = PgInterval {
            months: 0,
            days: 1,
            microseconds: 1_000_000,
        };
        let result = interval_to_duration(interval).unwrap();
        assert_eq!(result, Duration::from_secs(86401));
    }

    #[test]
    fn test_with_microseconds_conversion() {
        const MICROS: i64 = 1_123_456;
        let interval = PgInterval {
            months: 0,
            days: 0,
            microseconds: MICROS,
        };
        let result = interval_to_duration(interval).unwrap();
        assert_eq!(result, Duration::from_micros(MICROS as u64));
    }
}
