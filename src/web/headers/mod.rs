mod canonical_url;
mod surrogate_key;

pub use canonical_url::CanonicalUrl;
use http::HeaderName;
pub use surrogate_key::{SURROGATE_KEY, SurrogateKey, SurrogateKeys};

/// Fastly's Surrogate-Control header
/// https://www.fastly.com/documentation/reference/http/http-headers/Surrogate-Control/
pub static SURROGATE_CONTROL: HeaderName = HeaderName::from_static("surrogate-control");
