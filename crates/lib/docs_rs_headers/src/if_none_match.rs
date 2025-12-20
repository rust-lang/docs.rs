//! Adapted version of `headers::IfNoneMatch`.
//!
//! The combination of `TypedHeader` and `IfNoneMatch` works in odd ways.
//! They are built in a way that a _missing_ `If-None-Match` header will lead to:
//!
//! 1. extractor with `TypedHeader<IfNoneMatch>` returning `IfNoneMatch("")`
//! 2. extractor with `Option<TypedHeader<IfNoneMatch>>` returning `Some(IfNoneMatch(""))`
//!
//! Where I would expect:
//! 1. a failure because of the missing header
//! 2. `None` for the missing header
//!
//! This could be solved by either adapting `TypedHeader` or `IfNoneMatch`, I'm not sure which is
//! right.
//!
//! Some reading material for those interested:
//! * https://github.com/hyperium/headers/issues/204
//! * https://github.com/hyperium/headers/pull/165
//! * https://github.com/tokio-rs/axum/issues/1781
//! * https://github.com/tokio-rs/axum/pull/1810
//! * https://github.com/tokio-rs/axum/pull/2475
//!
//! Right now I feel like adapting `IfNoneMatch` is the "most correct-ish" option.

#[allow(clippy::disallowed_types)]
mod header_impl {
    use derive_more::Deref;
    use headers::{self, ETag, Header, IfNoneMatch as OriginalIfNoneMatch};

    #[derive(Debug, Clone, PartialEq, Deref)]
    pub struct IfNoneMatch(pub headers::IfNoneMatch);

    impl Header for IfNoneMatch {
        fn name() -> &'static http::HeaderName {
            OriginalIfNoneMatch::name()
        }

        fn decode<'i, I>(values: &mut I) -> Result<Self, headers::Error>
        where
            Self: Sized,
            I: Iterator<Item = &'i http::HeaderValue>,
        {
            let mut values = values.peekable();

            // NOTE: this is the difference to the original implementation.
            // When there is no header in the request, I want the decoding to fail.
            // This makes Option<TypedHeader<H>> return `None`, and also matches
            // most other header implementations.
            if values.peek().is_none() {
                Err(headers::Error::invalid())
            } else {
                OriginalIfNoneMatch::decode(&mut values).map(IfNoneMatch)
            }
        }

        fn encode<E: Extend<http::HeaderValue>>(&self, values: &mut E) {
            self.0.encode(values)
        }
    }

    impl From<ETag> for IfNoneMatch {
        fn from(value: ETag) -> Self {
            Self(value.into())
        }
    }
}

pub use header_impl::IfNoneMatch;

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use axum_core::{RequestPartsExt as _, body::Body, extract::Request};
    use axum_extra::{
        TypedHeader,
        headers::{ETag, HeaderMapExt as _},
    };
    use http::{HeaderMap, request};

    fn parts(if_none_match: Option<IfNoneMatch>) -> request::Parts {
        let mut builder = Request::builder();

        if let Some(if_none_match) = if_none_match {
            let headers = builder.headers_mut().unwrap();
            headers.typed_insert(if_none_match.clone());
        }

        let (parts, _body) = builder.uri("/").body(Body::empty()).unwrap().into_parts();

        parts
    }

    fn example_header() -> IfNoneMatch {
        IfNoneMatch::from("\"some-etag-value\"".parse::<ETag>().unwrap())
    }

    #[test]
    fn test_normal_typed_get_with_empty_headers() {
        let map = HeaderMap::new();
        assert!(map.typed_get::<IfNoneMatch>().is_none());
        assert!(map.typed_try_get::<IfNoneMatch>().unwrap().is_none());
    }

    #[test]
    fn test_normal_typed_get_with_value_headers() -> Result<()> {
        let if_none_match = example_header();

        let mut map = HeaderMap::new();
        map.typed_insert(if_none_match.clone());

        assert_eq!(map.typed_get::<IfNoneMatch>(), Some(if_none_match.clone()));
        assert_eq!(map.typed_try_get::<IfNoneMatch>()?, Some(if_none_match));

        Ok(())
    }

    #[tokio::test]
    async fn test_extract_from_empty_request_via_optional_typed_header() -> Result<()> {
        let mut parts = parts(None);

        assert!(
            parts
                .extract::<Option<TypedHeader<IfNoneMatch>>>()
                .await?
                // this is what we want, and the default `headers::IfNoneMatch` header can't
                // offer. Or the impl of  the `TypedHeader` extractor, depending on
                // interpretation.
                .is_none()
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_extract_from_empty_request_via_mandatory_typed_header() -> Result<()> {
        let mut parts = parts(None);

        // mandatory extractor leads to error when the header is missing.
        assert!(parts.extract::<TypedHeader<IfNoneMatch>>().await.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_extract_from_header_via_optional_typed_header() -> Result<()> {
        let if_none_match = example_header();
        let mut parts = parts(Some(if_none_match.clone()));

        assert_eq!(
            parts
                .extract::<Option<TypedHeader<IfNoneMatch>>>()
                .await?
                .map(|th| th.0),
            Some(if_none_match)
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_extract_from_header_via_mandatory_typed_header() -> Result<()> {
        let if_none_match = example_header();
        let mut parts = parts(Some(if_none_match.clone()));

        assert_eq!(
            parts.extract::<TypedHeader<IfNoneMatch>>().await?.0,
            if_none_match
        );

        Ok(())
    }
}
