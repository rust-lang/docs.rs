mod canonical_url;
mod if_none_match;
mod surrogate_key;

use axum_extra::headers::ETag;
use http::HeaderName;
use std::io::{self, Write};

pub use canonical_url::CanonicalUrl;
pub(crate) use if_none_match::IfNoneMatch;
pub use surrogate_key::{SURROGATE_KEY, SurrogateKey, SurrogateKeys};

/// Fastly's Surrogate-Control header
/// https://www.fastly.com/documentation/reference/http/http-headers/Surrogate-Control/
pub static SURROGATE_CONTROL: HeaderName = HeaderName::from_static("surrogate-control");

/// X-Robots-Tag header for search engines.
pub static X_ROBOTS_TAG: HeaderName = HeaderName::from_static("x-robots-tag");
