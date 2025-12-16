use anyhow::Result;
use askama::filters::HtmlSafe;
use axum_extra::headers::{Header, HeaderName, HeaderValue};
use docs_rs_uri::EscapedURI;
use http::uri::Uri;
use serde::Serialize;
use std::{fmt, ops::Deref};

/// simplified typed header for a `Link rel=canonical` header in the response.
///
/// When given only a path, it builds a full docs.rs URL.
#[derive(Debug, Clone)]
pub struct CanonicalUrl(EscapedURI);

impl CanonicalUrl {
    pub fn from_uri(uri: EscapedURI) -> Self {
        if uri.scheme().is_some() && uri.authority().is_some() {
            return Self(uri);
        }

        let mut parts = uri.into_inner().into_parts();

        if parts.scheme.is_none() {
            parts.scheme = Some("https".try_into().unwrap());
        }

        if parts.authority.is_none() {
            parts.authority = Some("docs.rs".try_into().unwrap());
        }

        Self(EscapedURI::from_uri(
            Uri::from_parts(parts).expect("parts were already in Uri, or are static"),
        ))
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
        let value: HeaderValue = format!(r#"<{}>; rel="canonical""#, self.0).parse().unwrap();

        values.extend(std::iter::once(value));
    }
}

impl fmt::Display for CanonicalUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for CanonicalUrl {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl From<Uri> for CanonicalUrl {
    fn from(value: Uri) -> Self {
        Self(EscapedURI::from_uri(value))
    }
}

impl From<EscapedURI> for CanonicalUrl {
    fn from(value: EscapedURI) -> Self {
        Self::from_uri(value)
    }
}

impl Deref for CanonicalUrl {
    type Target = EscapedURI;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl HtmlSafe for CanonicalUrl {}

#[cfg(test)]
mod tests {
    use super::*;
    use axum_extra::headers::HeaderMapExt;
    use http::HeaderMap;

    #[test]
    fn test_serialize_canonical_from_uri() {
        let url = CanonicalUrl::from_uri(EscapedURI::from_uri(
            Uri::builder()
                .scheme("https")
                .authority("some_server.org")
                .path_and_query("/some/path.html")
                .build()
                .unwrap(),
        ));

        assert_eq!(
            serde_json::to_string(&url).unwrap(),
            "\"https://some_server.org/some/path.html\""
        );
    }

    #[test]
    fn test_serialize_canonical() {
        let url = CanonicalUrl::from_uri("/some/path/".parse::<Uri>().unwrap().into());

        assert_eq!(
            serde_json::to_string(&url).unwrap(),
            "\"https://docs.rs/some/path/\""
        );
    }

    #[test]
    fn test_encode_canonical() {
        let mut map = HeaderMap::new();
        map.typed_insert(CanonicalUrl::from_uri(
            "/some/path/".parse::<Uri>().unwrap().into(),
        ));
        assert_eq!(
            map["link"],
            "<https://docs.rs/some/path/>; rel=\"canonical\""
        );
    }

    #[test]
    fn test_encode_canonical_with_encoding() {
        // umlauts are allowed in http::Uri, but we still want to encode them.
        let mut map = HeaderMap::new();
        map.typed_insert(CanonicalUrl::from_uri(
            "/some/äöü/".parse::<Uri>().unwrap().into(),
        ));
        assert_eq!(
            map["link"],
            "<https://docs.rs/some/%C3%A4%C3%B6%C3%BC/>; rel=\"canonical\""
        );
    }
}
