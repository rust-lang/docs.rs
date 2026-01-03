//! custom axum extractors for path parameters
use crate::error::AxumNope;
use anyhow::anyhow;
use axum::{
    RequestPartsExt,
    extract::{FromRequestParts, OptionalFromRequestParts},
    http::request::Parts,
};
use derive_more::Deref;
use docs_rs_types::{CompressionAlgorithm, compression_from_file_extension};

/// custom axum `Path` extractor that uses our own AxumNope::BadRequest
/// as error response instead of a plain text "bad request"
#[allow(clippy::disallowed_types)]
mod path_impl {
    use super::*;
    use serde::de::DeserializeOwned;

    #[derive(FromRequestParts)]
    #[from_request(via(axum::extract::Path), rejection(AxumNope))]
    pub(crate) struct Path<T>(pub T);

    impl<T, S> OptionalFromRequestParts<S> for Path<T>
    where
        T: DeserializeOwned + Send + 'static,
        S: Send + Sync,
    {
        type Rejection = AxumNope;

        async fn from_request_parts(
            parts: &mut Parts,
            _state: &S,
        ) -> Result<Option<Self>, Self::Rejection> {
            parts
                .extract::<Option<axum::extract::Path<T>>>()
                .await
                .map(|path| path.map(|obj| Path(obj.0)))
                .map_err(|err| AxumNope::BadRequest(err.into()))
        }
    }
}

pub(crate) use path_impl::Path;

impl From<axum::extract::rejection::PathRejection> for AxumNope {
    fn from(value: axum::extract::rejection::PathRejection) -> Self {
        AxumNope::BadRequest(value.into())
    }
}

/// extract a potential file extension from a path.
/// Axum doesn't support file extension suffixes yet,
/// especially when we have a route like '/something/{parameter}.{ext}' where two
/// parameters are used, one of which is a file extension.
///
/// This is already solved in matchit 0.8.6, but not yet in axum
/// https://github.com/ibraheemdev/matchit/issues/17
/// https://github.com/tokio-rs/axum/pull/3143
///
/// So our workaround is:
/// 1. we provide explicit routes for all file extensions we need to support (so no `.{ext}`).
/// 2. we extract the file extension from the path manually, using this extractor.
#[derive(Debug)]
pub(crate) struct PathFileExtension(pub(crate) String);

impl<S> FromRequestParts<S> for PathFileExtension
where
    S: Send + Sync,
{
    type Rejection = AxumNope;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extract::<Option<PathFileExtension>>()
            .await
            .expect("can never fail")
            .ok_or_else(|| AxumNope::BadRequest(anyhow!("file extension not found in path")))
    }
}

impl<S> OptionalFromRequestParts<S> for PathFileExtension
where
    S: Send + Sync,
{
    type Rejection = ();

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        if let Some((_rest, last_component)) = parts.uri.path().rsplit_once('/')
            && let Some((_rest, ext)) = last_component.rsplit_once('.')
        {
            return Ok(Some(PathFileExtension(ext.to_string())));
        }

        Ok(None)
    }
}

/// get wanted compression from file extension in path.
///
/// TODO: we could also additionally read the accept-encoding header here. But especially
/// in combination with priorities it's complex to parse correctly. So for now only
/// file extensions in the URL.
/// When using Accept-Encoding, we also have to return "Vary: Accept-Encoding" to ensure
/// the cache behaves correctly.
#[derive(Debug, Clone, Deref, Default, PartialEq)]
pub(crate) struct WantedCompression(pub(crate) CompressionAlgorithm);

impl<S> FromRequestParts<S> for WantedCompression
where
    S: Send + Sync,
{
    type Rejection = AxumNope;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extract::<Option<WantedCompression>>()
            .await
            .expect("can never fail")
            .ok_or_else(|| AxumNope::BadRequest(anyhow!("compression extension not found in path")))
    }
}

impl<S> OptionalFromRequestParts<S> for WantedCompression
where
    S: Send + Sync,
{
    type Rejection = AxumNope;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        if let Some(ext) = parts
            .extract::<Option<PathFileExtension>>()
            .await
            .expect("can't fail")
            .map(|ext| ext.0)
        {
            Ok(Some(WantedCompression(
                compression_from_file_extension(&ext).ok_or_else(|| {
                    AxumNope::BadRequest(anyhow!("unknown compression file extension: {}", ext))
                })?,
            )))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{AxumResponseTestExt, AxumRouterTestExt};
    use axum::{Router, routing::get};
    use http::StatusCode;

    #[tokio::test]
    async fn test_path_file_ext() -> anyhow::Result<()> {
        let app = Router::new()
            .route(
                "/mandatory/something.pdf",
                get(|PathFileExtension(ext): PathFileExtension| async move {
                    format!("mandatory: {ext}")
                }),
            )
            .route(
                "/mandatory_missing/something",
                get(|PathFileExtension(_ext): PathFileExtension| async move { "never called" }),
            )
            .route(
                "/",
                get(|PathFileExtension(_ext): PathFileExtension| async move { "never called" }),
            )
            .route(
                "/optional/something.pdf",
                get(|ext: Option<PathFileExtension>| async move { format!("option: {ext:?}") }),
            )
            .route(
                "/optional_missing/something",
                get(|ext: Option<PathFileExtension>| async move { format!("option: {ext:?}") }),
            );

        let res = app.get("/mandatory/something.pdf").await?;
        assert!(res.status().is_success());
        assert_eq!(res.text().await?, "mandatory: pdf");

        for path in &["/mandatory_missing/something", "/"] {
            let res = app.get(path).await?;
            assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        }

        let res = app.get("/optional/something.pdf").await?;
        assert!(res.status().is_success());
        assert_eq!(
            res.text().await?,
            "option: Some(PathFileExtension(\"pdf\"))"
        );

        let res = app.get("/optional_missing/something").await?;
        assert!(res.status().is_success());
        assert_eq!(res.text().await?, "option: None");

        Ok(())
    }
}
