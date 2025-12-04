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

/// compute our etag header value from some content
///
/// Has to match the implementation in our build-script.
pub fn compute_etag<T: AsRef<[u8]>>(content: T) -> ETag {
    let mut computer = ETagComputer::new();
    computer.write_all(content.as_ref()).unwrap();
    computer.finalize()
}

/// Helper type to compute ETag values.
///
/// Works the same way as the inner `md5::Context`,
/// but produces an `ETag` when finalized.
pub(crate) struct ETagComputer(md5::Context);

impl ETagComputer {
    pub fn new() -> Self {
        Self(md5::Context::new())
    }

    pub fn consume<T: AsRef<[u8]>>(&mut self, data: T) {
        self.0.consume(data.as_ref());
    }

    pub fn finalize(self) -> ETag {
        let digest = self.0.finalize();
        format!("\"{:x}\"", digest).parse().unwrap()
    }
}

impl io::Write for ETagComputer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
