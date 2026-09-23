use headers::CacheControl;
use std::time::Duration;

/// Compute remaining freshness using parsed cache directives and the response age.
/// This interprets `max-age` and `no-cache`; it is not a full HTTP
/// cache policy evaluator. Missing `max-age` returns `None`.
pub fn cache_control_ttl(control: Option<&CacheControl>, age: Duration) -> Option<Duration> {
    if control.is_some_and(CacheControl::no_cache) {
        return Some(Duration::ZERO);
    }
    control
        .and_then(CacheControl::max_age)
        .map(|ttl| ttl.saturating_sub(age))
}

#[cfg(test)]
mod tests {
    use super::*;
    use headers::HeaderMapExt;
    use http::HeaderMap;
    use test_case::test_case;

    #[test_case("max-age=600", 0, Some(600); "max age")]
    #[test_case("public, max-age=600", 86, Some(514); "subtract age")]
    #[test_case("max-age=600", 700, Some(0); "already stale")]
    #[test_case("no-cache, max-age=600", 0, Some(0); "no cache")]
    #[test_case("max-age=0", 0, Some(0); "zero")]
    #[test_case("", 0, None; "missing max age")]
    #[test_case("max-age=invalid", 0, None; "invalid max age")]
    fn parses_ttl(control: &str, age: u64, seconds: Option<u64>) {
        let mut headers = HeaderMap::new();
        headers.insert(http::header::CACHE_CONTROL, control.parse().unwrap());
        let expected = seconds.map(Duration::from_secs);
        assert_eq!(
            cache_control_ttl(
                headers.typed_get::<CacheControl>().as_ref(),
                Duration::from_secs(age)
            ),
            expected
        );
    }

    #[test]
    fn absent_headers_leave_fallback_to_caller() {
        assert_eq!(cache_control_ttl(None, Duration::ZERO), None);
    }

    #[test]
    fn handles_multiple_header_values_and_absent_age() {
        let mut headers = HeaderMap::new();
        headers.append(http::header::CACHE_CONTROL, "public".parse().unwrap());
        headers.append(http::header::CACHE_CONTROL, "max-age=600".parse().unwrap());
        assert_eq!(
            cache_control_ttl(headers.typed_get::<CacheControl>().as_ref(), Duration::ZERO),
            Some(Duration::from_secs(600))
        );
    }
}
