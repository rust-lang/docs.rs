use http::StatusCode;
use reqwest_middleware::Error;
use reqwest_retry::{DefaultRetryableStrategy, Retryable, RetryableStrategy};

/// Stop immediately when the registry rate limits us; retry other transient failures.
pub(crate) struct RegistryApiRetryStrategy;

impl RetryableStrategy for RegistryApiRetryStrategy {
    fn handle(&self, result: &std::result::Result<reqwest::Response, Error>) -> Option<Retryable> {
        match result {
            Ok(response) if response.status() == StatusCode::TOO_MANY_REQUESTS => {
                Some(Retryable::Fatal)
            }
            _ => DefaultRetryableStrategy.handle(result),
        }
    }
}
