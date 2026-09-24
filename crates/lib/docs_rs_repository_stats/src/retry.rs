use reqwest::StatusCode;
use reqwest_middleware::Error as MiddlewareError;
use reqwest_retry::{DefaultRetryableStrategy, Retryable, RetryableStrategy};

/// Retry policy for repository-forge APIs.
///
/// Repo-Stats run as a scheduled task once an hour. When we reach a rate limit
/// we just stop handling repos for this run, and continue at the next scheduled time.
///
/// Also, some HTTP statuses are treated as fatal in `DefaultRetryableStrategy`, while
/// we think they are actually transient.
pub(crate) struct RepositoryForgeRetryStrategy;

impl RetryableStrategy for RepositoryForgeRetryStrategy {
    fn handle(&self, result: &Result<reqwest::Response, MiddlewareError>) -> Option<Retryable> {
        match result {
            Ok(response) if response.status() == StatusCode::TOO_MANY_REQUESTS => {
                Some(Retryable::Fatal)
            }
            Ok(response) if response.status().as_u16() == 499 => {
                // NGINX defines a non-standard HTTP status code:
                // `NGX_HTTP_CLIENT_CLOSED_REQUEST     499`
                // See:
                // https://en.wikipedia.org/wiki/List_of_HTTP_status_codes#nginx
                // https://web.archive.org/web/20170919111558/http://lxr.nginx.org/source/src/http/ngx_http_request.h
                //
                // We believe retrying is safe because the server did not complete the request.
                Some(Retryable::Transient)
            }
            _ => DefaultRetryableStrategy.handle(result),
        }
    }
}
