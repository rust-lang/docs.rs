use anyhow::{Result, bail};
use axum::{
    extract::Request as AxumHttpRequest,
    middleware::Next,
    response::{IntoResponse as _, Response as AxumResponse},
};
use docs_rs_uri::url_decode;
use http::{StatusCode, Uri};
use std::borrow::Cow;
use tracing::warn;

const MAX_DECODE_PASSES: usize = 3;

pub(crate) async fn security_middleware(
    uri: Uri,
    req: AxumHttpRequest,
    next: Next,
) -> AxumResponse {
    if let Err(err) = validate_path(uri.path()) {
        warn!(%uri, ?err, "detected blocked request path");
        return StatusCode::NOT_ACCEPTABLE.into_response();
    }

    next.run(req).await
}

fn validate_path(initial_path: &str) -> Result<()> {
    let mut path = Cow::Borrowed(initial_path);
    for _ in 0..MAX_DECODE_PASSES {
        validate_decoded_path(path.as_ref())?;

        match url_decode(path.as_ref())? {
            Cow::Borrowed(_) => break,
            Cow::Owned(decoded) => path = Cow::Owned(decoded),
        }
    }

    validate_decoded_path(path.as_ref())?;

    Ok(())
}

fn validate_decoded_path(path: &str) -> Result<()> {
    if path.contains("/../") || path.ends_with("/..") {
        bail!("path traversal attempt");
    }

    // `#` is never allowed in any rustdoc URLs, even encoded.
    if path.contains('#') {
        bail!("detected `#` in request path");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        extractors::Path,
        testing::{AxumResponseTestExt as _, AxumRouterTestExt as _},
    };
    use axum::{Router, middleware, routing::get};
    use test_case::test_case;
    use tower::ServiceBuilder;

    #[tokio::test]
    #[test_case("/%80"; "invalid UTF8, continuation byte without a leading byte")]
    #[test_case("/../"; "relative path with slash")]
    #[test_case("/.."; "relative path")]
    #[test_case("/asdf/../"; "relative path 2")]
    #[test_case("/tiny_http/latest/tiny_http%2f%2e%2e"; "encoded")]
    #[test_case("/tiny_http/latest/tiny_http%252f%252e%252e"; "double encoded traversal")]
    #[test_case("/tiny_http/latest/tiny_http%25252f%25252e%25252e"; "triple encoded traversal")]
    #[test_case("/minidumper/latest/%23%3c%2f%73%63%72%69%70%74%3e%3c%74%65%73%74%65%3e"; "encoded XSS probe")]
    #[test_case("/minidumper/latest/%2523script"; "double encoded hash")]
    #[test_case("/minidumper/latest/%252523script"; "triple encoded hash")]
    #[test_case(
        "/crate/mika-cli/latest/source/..%25c1%259c..%25c1%259c..%25c1%259c..%25c1%259c..%25c1%259c..%25c1%259c..%25c1%259c..%25c1%259c/etc/passwd"
    )]
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
