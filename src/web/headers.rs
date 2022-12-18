use axum::{
    headers::{Header, HeaderName, HeaderValue},
    http::uri::Uri,
};

/// simplified typed header for a `Link rel=canonical` header in the response.
pub struct CanonicalUrl(pub Uri);

impl Header for CanonicalUrl {
    fn name() -> &'static HeaderName {
        &http::header::LINK
    }

    fn decode<'i, I>(_values: &mut I) -> Result<Self, axum::headers::Error>
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

#[cfg(test)]
mod tests {
    use super::*;

    use axum::headers::HeaderMapExt;
    use axum::http::HeaderMap;

    #[test]
    fn test_encode_canonical() {
        let mut map = HeaderMap::new();
        map.typed_insert(CanonicalUrl("http://something/".parse().unwrap()));
        assert_eq!(map["link"], "<http://something/>; rel=\"canonical\"");
    }
}
