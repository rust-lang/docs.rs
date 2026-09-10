use reqwest::StatusCode;
use reqwest_middleware::Error as MiddlewareError;
use reqwest_retry::{DefaultRetryableStrategy, Retryable, RetryableStrategy};

/// Retries transient failures, except rate limits, which callers must handle immediately.
///
/// Repo-Stats run as a scheduled task once an hour. When we reach a rate limit
/// we just stop handling repos for this run, and continue at the next scheduled time.
pub(crate) struct NoRateLimitRetryStrategy;

impl RetryableStrategy for NoRateLimitRetryStrategy {
    fn handle(&self, result: &Result<reqwest::Response, MiddlewareError>) -> Option<Retryable> {
        if let Ok(response) = result
            && response.status() == StatusCode::TOO_MANY_REQUESTS
        {
            Some(Retryable::Fatal)
        } else {
            DefaultRetryableStrategy.handle(result)
        }
    }
}
