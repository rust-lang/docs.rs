use axum::{
    extract::Request as AxumHttpRequest,
    middleware::Next,
    response::{IntoResponse as _, Response as AxumResponse},
};
use docs_rs_uri::url_decode;
use http::{StatusCode, Uri};
use tracing::warn;

pub(crate) async fn security_middleware(
    uri: Uri,
    req: AxumHttpRequest,
    next: Next,
) -> AxumResponse {
    let path = uri.path();

    if let Err(err) = url_decode(path) {
        warn!(%uri, ?err, "invalid UTF-8 in request path");
        return StatusCode::NOT_ACCEPTABLE.into_response();
    }

    if path.contains("/../") || path.ends_with("/..") {
        warn!(%uri, "detected path traversal attempt");
        return StatusCode::NOT_ACCEPTABLE.into_response();
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        extractors::Path,
        testing::{AxumResponseTestExt as _, AxumRouterTestExt as _},
    };
    use anyhow::Result;
    use axum::{Router, middleware, routing::get};
    use test_case::test_case;
    use tower::ServiceBuilder;

    #[tokio::test]
    #[test_case("/%80"; "invalid UTF8, continuation byte without a leading byte")]
    #[test_case("/../"; "relative path with slash")]
    #[test_case("/.."; "relative path")]
    #[test_case("/asdf/../"; "relative path 2")]
    async fn test_invalid_path(path: &str) -> Result<()> {
        let app = Router::new()
            .route("/{*inner}", get(|| async { StatusCode::OK }))
            .layer(ServiceBuilder::new().layer(middleware::from_fn(security_middleware)));

        let response = app.get(path).await?;
        assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);
        assert!(response.text().await?.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_pass() -> Result<()> {
        let app = Router::new()
            .route(
                "/{*inner}",
                get(|Path(inner): Path<String>| async { inner }),
            )
            .layer(ServiceBuilder::new().layer(middleware::from_fn(security_middleware)));

        let response = app.assert_success("/some/path").await?;
        assert_eq!(response.text().await?, "some/path");

        Ok(())
    }
}
