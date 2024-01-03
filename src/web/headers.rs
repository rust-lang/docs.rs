use super::encode_url_path;
use anyhow::Result;
use axum::http::uri::{PathAndQuery, Uri};
use axum_extra::headers::{Header, HeaderName, HeaderValue};
use serde::Serialize;

/// simplified typed header for a `Link rel=canonical` header in the response.
/// Only takes the path to be used, url-encodes it and attaches domain & schema to it.
#[derive(Debug, Clone)]
pub struct CanonicalUrl(PathAndQuery);

impl CanonicalUrl {
    pub fn from_path<P: AsRef<str>>(path: P) -> Self {
        Self(
            encode_url_path(path.as_ref())
                .try_into()
                .expect("invalid URI path characters even after encoding them"),
        )
    }

    fn build_full_uri(&self) -> Uri {
        Uri::builder()
            .scheme("https")
            .authority("docs.rs")
            .path_and_query(self.0.clone())
            .build()
            .expect("this unwrap can't fail because PathAndQuery is valid")
    }
}

impl Header for CanonicalUrl {
    fn name() -> &'static HeaderName {
        &http::header::LINK
    }

    fn decode<'i, I>(_values: &mut I) -> Result<Self, axum_extra::headers::Error>
    where
        I: Iterator<Item = &'i HeaderValue>,
    {
        unimplemented!();
    }

    fn encode<E>(&self, values: &mut E)
    where
        E: Extend<HeaderValue>,
    {
        let value: HeaderValue = format!(r#"<{}>; rel="canonical""#, self.build_full_uri())
            .parse()
            .unwrap();

        values.extend(std::iter::once(value));
    }
}

impl Serialize for CanonicalUrl {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.build_full_uri().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::http::HeaderMap;
    use axum_extra::headers::HeaderMapExt;

    #[test]
    fn test_serialize_canonical() {
        let url = CanonicalUrl::from_path("/some/path/");

        assert_eq!(
            serde_json::to_string(&url).unwrap(),
            "\"https://docs.rs/some/path/\""
        );
    }

    #[test]
    fn test_encode_canonical() {
        let mut map = HeaderMap::new();
        map.typed_insert(CanonicalUrl::from_path("/some/path/"));
        assert_eq!(
            map["link"],
            "<https://docs.rs/some/path/>; rel=\"canonical\""
        );
    }

    #[test]
    fn test_encode_canonical_with_encoding() {
        let mut map = HeaderMap::new();
        map.typed_insert(CanonicalUrl::from_path("/some/äöü/"));
        assert_eq!(
            map["link"],
            "<https://docs.rs/some/%C3%A4%C3%B6%C3%BC/>; rel=\"canonical\""
        );
    }
}
