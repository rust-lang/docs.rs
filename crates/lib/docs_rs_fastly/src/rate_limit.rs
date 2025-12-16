use chrono::{DateTime, TimeZone as _, Utc};
use http::{HeaderMap, HeaderName};

// https://www.fastly.com/documentation/reference/api/#rate-limiting
pub(crate) const FASTLY_RATELIMIT_REMAINING: HeaderName =
    HeaderName::from_static("fastly-ratelimit-remaining");
pub(crate) const FASTLY_RATELIMIT_RESET: HeaderName =
    HeaderName::from_static("fastly-ratelimit-reset");

pub(crate) fn fetch_rate_limit_state(headers: &HeaderMap) -> (Option<u64>, Option<DateTime<Utc>>) {
    // https://www.fastly.com/documentation/reference/api/#rate-limiting
    (
        headers
            .get(FASTLY_RATELIMIT_REMAINING)
            .and_then(|hv| hv.to_str().ok())
            .and_then(|s| s.parse().ok()),
        headers
            .get(FASTLY_RATELIMIT_RESET)
            .and_then(|hv| hv.to_str().ok())
            .and_then(|s| s.parse::<i64>().ok())
            .and_then(|ts| Utc.timestamp_opt(ts, 0).single()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use http::HeaderValue;

    #[test]
    fn test_read_rate_limit() {
        // https://www.fastly.com/documentation/reference/api/#rate-limiting
        let mut hm = HeaderMap::new();
        hm.insert(FASTLY_RATELIMIT_REMAINING, HeaderValue::from_static("999"));
        hm.insert(
            FASTLY_RATELIMIT_RESET,
            HeaderValue::from_static("1452032384"),
        );

        let (remaining, reset) = fetch_rate_limit_state(&hm);
        assert_eq!(remaining, Some(999));
        assert_eq!(
            reset,
            Some(Utc.timestamp_opt(1452032384, 0).single().unwrap())
        );
    }
}
