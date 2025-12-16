#[derive(Debug, thiserror::Error)]
#[error("rate limit reached")]
pub struct RateLimitReached;
