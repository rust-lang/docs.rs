mod canonical_url;
mod if_none_match;
mod surrogate_key;

pub use canonical_url::CanonicalUrl;
use http::HeaderName;
pub(crate) use if_none_match::IfNoneMatch;
pub use surrogate_key::{SURROGATE_KEY, SurrogateKey, SurrogateKeys};

/// Fastly's Surrogate-Control header
/// https://www.fastly.com/documentation/reference/http/http-headers/Surrogate-Control/
pub static SURROGATE_CONTROL: HeaderName = HeaderName::from_static("surrogate-control");

/// compute our etag header value from some content
///
/// Has to match the implementation in our build-script.
#[cfg(test)]
pub fn compute_etag<T: AsRef<[u8]>>(content: T) -> axum_extra::headers::ETag {
    let digest = md5::compute(&content);
    format!("\"{:x}\"", digest).parse().unwrap()
}
