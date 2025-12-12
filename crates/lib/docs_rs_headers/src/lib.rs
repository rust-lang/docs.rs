pub mod etag;
pub use headers::ETag;

mod canonical_url;
mod if_none_match;
mod surrogate_key;

pub use canonical_url::CanonicalUrl;
use http::HeaderName;
pub use if_none_match::IfNoneMatch;
pub use surrogate_key::{SURROGATE_KEY, SurrogateKey, SurrogateKeys};

/// Fastly's Surrogate-Control header
/// https://www.fastly.com/documentation/reference/http/http-headers/Surrogate-Control/
pub static SURROGATE_CONTROL: HeaderName = HeaderName::from_static("surrogate-control");

/// X-Robots-Tag header for search engines.
pub static X_ROBOTS_TAG: HeaderName = HeaderName::from_static("x-robots-tag");
