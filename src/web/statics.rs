use super::{cache::CachePolicy, metrics::request_recorder, routes::get_static};
use axum::{
    Router as AxumRouter,
    extract::{Extension, Request},
    http::header::CONTENT_TYPE,
    middleware,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get_service,
};
use axum_extra::headers::HeaderValue;
use tower_http::services::ServeDir;

const VENDORED_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/vendored.css"));
const STYLE_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/style.css"));
const RUSTDOC_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/rustdoc.css"));
const RUSTDOC_2021_12_05_CSS: &str =
    include_str!(concat!(env!("OUT_DIR"), "/rustdoc-2021-12-05.css"));
const RUSTDOC_2025_08_20_CSS: &str =
    include_str!(concat!(env!("OUT_DIR"), "/rustdoc-2025-08-20.css"));

fn build_static_css_response(content: &'static str) -> impl IntoResponse {
    (
        Extension(CachePolicy::ForeverInCdnAndBrowser),
        [(CONTENT_TYPE, mime::TEXT_CSS.as_ref())],
        content,
    )
}

async fn set_needed_static_headers(req: Request, next: Next) -> Response {
    let req_path = req.uri().path();
    let is_opensearch_xml = req_path.ends_with("/opensearch.xml");

    let mut response = next.run(req).await;

    if response.status().is_success() {
        response
            .extensions_mut()
            .insert(CachePolicy::ForeverInCdnAndBrowser);
    }

    if is_opensearch_xml {
        // overwrite the content type for opensearch.xml,
        // otherwise mime-guess would return `text/xml`.
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/opensearchdescription+xml"),
        );
    }

    response
}

pub(crate) fn build_static_router() -> AxumRouter {
    AxumRouter::new()
        .route(
            "/vendored.css",
            get_static(|| async { build_static_css_response(VENDORED_CSS) }),
        )
        .route(
            "/style.css",
            get_static(|| async { build_static_css_response(STYLE_CSS) }),
        )
        .route(
            "/rustdoc.css",
            get_static(|| async { build_static_css_response(RUSTDOC_CSS) }),
        )
        .route(
            "/rustdoc-2021-12-05.css",
            get_static(|| async { build_static_css_response(RUSTDOC_2021_12_05_CSS) }),
        )
        .route(
            "/rustdoc-2025-08-20.css",
            get_static(|| async { build_static_css_response(RUSTDOC_2025_08_20_CSS) }),
        )
        .fallback_service(
            get_service(ServeDir::new("static").fallback(ServeDir::new("vendor")))
                .layer(middleware::from_fn(set_needed_static_headers))
                .layer(middleware::from_fn(|request, next| async {
                    request_recorder(request, next, Some("static resource")).await
                })),
        )
}

#[cfg(test)]
mod tests {
    use super::{STYLE_CSS, VENDORED_CSS};
    use crate::{
        test::{AxumResponseTestExt, AxumRouterTestExt, async_wrapper},
        web::cache::CachePolicy,
    };
    use axum::response::Response as AxumResponse;
    use reqwest::StatusCode;
    use std::fs;
    use test_case::test_case;

    const STATIC_SEARCH_PATHS: &[&str] = &["static", "vendor"];

    fn content_length(resp: &AxumResponse) -> u64 {
        resp.headers()
            .get("Content-Length")
            .expect("content-length header")
            .to_str()
            .unwrap()
            .parse()
            .unwrap()
    }

    #[test]
    fn style_css() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            let resp = web.get("/-/static/style.css").await?;
            assert!(resp.status().is_success());
            resp.assert_cache_control(CachePolicy::ForeverInCdnAndBrowser, &env.config());
            assert_eq!(
                resp.headers().get("Content-Type"),
                Some(&"text/css".parse().unwrap()),
            );
            assert_eq!(content_length(&resp), STYLE_CSS.len() as u64);
            assert_eq!(resp.bytes().await?, STYLE_CSS.as_bytes());

            Ok(())
        });
    }

    #[test]
    fn vendored_css() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            let resp = web.get("/-/static/vendored.css").await?;
            assert!(resp.status().is_success());
            resp.assert_cache_control(CachePolicy::ForeverInCdnAndBrowser, &env.config());
            assert_eq!(
                resp.headers().get("Content-Type"),
                Some(&"text/css".parse().unwrap()),
            );
            assert_eq!(content_length(&resp), VENDORED_CSS.len() as u64);
            assert_eq!(resp.text().await?, VENDORED_CSS);

            Ok(())
        });
    }

    #[test]
    fn io_error_not_a_directory_leads_to_404() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            // just to be sure that `index.js` exists
            assert!(web.get("/-/static/index.js").await?.status().is_success());

            // `index.js` exists, but is not a directory,
            // so trying to fetch it via `ServeDir` will lead
            // to an IO-error.
            let resp = web.get("/-/static/index.js/something").await?;
            assert_eq!(resp.status().as_u16(), StatusCode::NOT_FOUND);

            Ok(())
        });
    }

    #[test_case("/-/static/index.js", "resetClipboardTimeout")]
    #[test_case("/-/static/menu.js", "closeMenu")]
    #[test_case("/-/static/keyboard.js", "handleKey")]
    #[test_case("/-/static/source.js", "toggleSource")]
    fn js_content(path: &str, expected_content: &str) {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            let resp = web.get(path).await?;
            assert!(resp.status().is_success());
            resp.assert_cache_control(CachePolicy::ForeverInCdnAndBrowser, &env.config());
            assert_eq!(
                resp.headers().get("Content-Type"),
                Some(&"text/javascript".parse().unwrap()),
            );
            assert!(content_length(&resp) > 10);
            assert!(resp.text().await?.contains(expected_content));

            Ok(())
        });
    }

    #[test]
    fn static_files() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            for root in STATIC_SEARCH_PATHS {
                for entry in walkdir::WalkDir::new(root) {
                    let entry = entry?;
                    if !entry.file_type().is_file() {
                        continue;
                    }
                    let file = entry.path().strip_prefix(root).unwrap();
                    let path = entry.path();

                    let url = format!("/-/static/{}", file.to_str().unwrap());
                    let resp = web.get(&url).await?;

                    assert!(resp.status().is_success(), "failed to fetch {url:?}");
                    resp.assert_cache_control(CachePolicy::ForeverInCdnAndBrowser, &env.config());
                    assert_eq!(
                        resp.bytes().await?,
                        fs::read(path).unwrap(),
                        "failed to fetch {url:?}",
                    );
                }
            }

            Ok(())
        });
    }

    #[test]
    fn static_file_that_doesnt_exist() {
        async_wrapper(|env| async move {
            let response = env.web_app().await.get("/-/static/whoop-de-do.png").await?;
            response.assert_cache_control(CachePolicy::NoCaching, &env.config());
            assert_eq!(response.status(), StatusCode::NOT_FOUND);

            Ok(())
        });
    }

    #[test]
    fn static_mime_types() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            let files = &[("vendored.css", "text/css")];

            for (file, mime) in files {
                let url = format!("/-/static/{file}");
                let resp = web.get(&url).await?;

                assert_eq!(
                    resp.headers().get("Content-Type"),
                    Some(&mime.parse().unwrap()),
                    "{url:?} has an incorrect content type",
                );
            }

            Ok(())
        });
    }
}
