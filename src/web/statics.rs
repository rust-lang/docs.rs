use super::{cache::CachePolicy, metrics::request_recorder, routes::get_static};
use axum::{
    extract::{Extension, Request},
    http::header::CONTENT_TYPE,
    middleware,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get_service,
    Router as AxumRouter,
};
use axum_extra::headers::HeaderValue;
use tower_http::services::ServeDir;

const VENDORED_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/vendored.css"));
const STYLE_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/style.css"));
const RUSTDOC_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/rustdoc.css"));
const RUSTDOC_2021_12_05_CSS: &str =
    include_str!(concat!(env!("OUT_DIR"), "/rustdoc-2021-12-05.css"));

fn build_static_css_response(content: &'static str) -> impl IntoResponse {
    (
        Extension(CachePolicy::ForeverInCdnAndBrowser),
        [(CONTENT_TYPE, mime::TEXT_CSS.as_ref())],
        content,
    )
}

async fn set_needed_static_headers(req: Request, next: Next) -> Response {
    let is_opensearch_xml = req.uri().path().ends_with("/opensearch.xml");

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
        .nest_service(
            "/",
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
        test::{assert_cache_control, wrapper},
        web::cache::CachePolicy,
    };
    use reqwest::StatusCode;
    use std::fs;
    use test_case::test_case;

    const STATIC_SEARCH_PATHS: &[&str] = &["static", "vendor"];

    #[test]
    fn style_css() {
        wrapper(|env| {
            let web = env.frontend();

            let resp = web.get("/-/static/style.css").send()?;
            assert!(resp.status().is_success());
            assert_cache_control(&resp, CachePolicy::ForeverInCdnAndBrowser, &env.config());
            assert_eq!(
                resp.headers().get("Content-Type"),
                Some(&"text/css".parse().unwrap()),
            );
            assert_eq!(resp.content_length().unwrap(), STYLE_CSS.len() as u64);
            assert_eq!(resp.bytes()?, STYLE_CSS.as_bytes());

            Ok(())
        });
    }

    #[test]
    fn vendored_css() {
        wrapper(|env| {
            let web = env.frontend();

            let resp = web.get("/-/static/vendored.css").send()?;
            assert!(resp.status().is_success());
            assert_cache_control(&resp, CachePolicy::ForeverInCdnAndBrowser, &env.config());
            assert_eq!(
                resp.headers().get("Content-Type"),
                Some(&"text/css".parse().unwrap()),
            );
            assert_eq!(resp.content_length().unwrap(), VENDORED_CSS.len() as u64);
            assert_eq!(resp.text()?, VENDORED_CSS);

            Ok(())
        });
    }

    #[test]
    fn io_error_not_a_directory_leads_to_404() {
        wrapper(|env| {
            let web = env.frontend();

            // just to be sure that `index.js` exists
            assert!(web.get("/-/static/index.js").send()?.status().is_success());

            // `index.js` exists, but is not a directory,
            // so trying to fetch it via `ServeDir` will lead
            // to an IO-error.
            let resp = web.get("/-/static/index.js/something").send()?;
            assert_eq!(resp.status().as_u16(), StatusCode::NOT_FOUND);

            Ok(())
        });
    }

    #[test_case("/-/static/index.js", "resetClipboardTimeout")]
    #[test_case("/-/static/menu.js", "closeMenu")]
    #[test_case("/-/static/keyboard.js", "handleKey")]
    #[test_case("/-/static/source.js", "toggleSource")]
    fn js_content(path: &str, expected_content: &str) {
        wrapper(|env| {
            let web = env.frontend();

            let resp = web.get(path).send()?;
            assert!(resp.status().is_success());
            assert_cache_control(&resp, CachePolicy::ForeverInCdnAndBrowser, &env.config());
            assert_eq!(
                resp.headers().get("Content-Type"),
                Some(&"text/javascript".parse().unwrap()),
            );
            assert!(resp.content_length().unwrap() > 10);
            assert!(resp.text()?.contains(expected_content));

            Ok(())
        });
    }

    #[test]
    fn static_files() {
        wrapper(|env| {
            let web = env.frontend();

            for root in STATIC_SEARCH_PATHS {
                for entry in walkdir::WalkDir::new(root) {
                    let entry = entry?;
                    if !entry.file_type().is_file() {
                        continue;
                    }
                    let file = entry.path().strip_prefix(root).unwrap();
                    let path = entry.path();

                    let url = format!("/-/static/{}", file.to_str().unwrap());
                    let resp = web.get(&url).send()?;

                    assert!(resp.status().is_success(), "failed to fetch {url:?}");
                    assert_cache_control(&resp, CachePolicy::ForeverInCdnAndBrowser, &env.config());
                    assert_eq!(
                        resp.bytes()?,
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
        wrapper(|env| {
            let response = env.frontend().get("/-/static/whoop-de-do.png").send()?;
            assert_cache_control(&response, CachePolicy::NoCaching, &env.config());
            assert_eq!(response.status(), StatusCode::NOT_FOUND);

            Ok(())
        });
    }

    #[test]
    fn static_mime_types() {
        wrapper(|env| {
            let web = env.frontend();

            let files = &[("vendored.css", "text/css")];

            for (file, mime) in files {
                let url = format!("/-/static/{file}");
                let resp = web.get(&url).send()?;

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
