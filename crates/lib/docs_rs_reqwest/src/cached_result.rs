use std::time::Duration;

/// A value and the remaining time it may be reused, including negative results.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CachedResult<T> {
    pub value: T,
    pub ttl: Duration,
}

impl<T> CachedResult<T> {
    /// Transform the value while preserving its remaining freshness.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> CachedResult<U> {
        CachedResult {
            value: f(self.value),
            ttl: self.ttl,
        }
    }
}
