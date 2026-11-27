use axum::{
    http::HeaderValue,
    response::{IntoResponseParts, ResponseParts},
};
use docs_rs_headers::X_ROBOTS_TAG;
use std::convert::Infallible;

/// Whether search engines may index a response.
///
/// Handlers add this to their response, like `(SearchIndexing::..., page)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchIndexing {
    Index,
    NoIndex,
}

impl SearchIndexing {
    /// Only `/latest/` URLs should end up in search engines, versioned URLs
    /// would compete with them.
    pub(crate) fn latest_only(is_latest_url: bool) -> Self {
        if is_latest_url {
            Self::Index
        } else {
            Self::NoIndex
        }
    }
}

impl IntoResponseParts for SearchIndexing {
    type Error = Infallible;

    fn into_response_parts(self, mut res: ResponseParts) -> Result<ResponseParts, Self::Error> {
        if self == Self::NoIndex {
            res.headers_mut()
                .insert(&X_ROBOTS_TAG, HeaderValue::from_static("noindex"));
        }
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse as _;

    #[test]
    fn latest_only() {
        let response = (SearchIndexing::latest_only(true), "").into_response();
        assert!(response.headers().get(&X_ROBOTS_TAG).is_none());

        let response = (SearchIndexing::latest_only(false), "").into_response();
        assert_eq!(response.headers().get(&X_ROBOTS_TAG).unwrap(), "noindex");
    }
}
