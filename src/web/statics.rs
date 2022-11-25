use super::{
    cache::CachePolicy,
    error::{AxumNope, AxumResult},
};
use crate::utils::report_error;
use anyhow::Context;
use axum::{
    extract::{Extension, Path},
    http::{
        header::{CONTENT_LENGTH, CONTENT_TYPE, LAST_MODIFIED},
        StatusCode,
    },
    response::{IntoResponse, Response},
};
use chrono::prelude::*;
use httpdate::fmt_http_date;
use mime::Mime;
use mime_guess::MimeGuess;
use std::{ffi::OsStr, path, time::SystemTime};
use tokio::fs;

const VENDORED_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/vendored.css"));
const STYLE_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/style.css"));
const RUSTDOC_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/rustdoc.css"));
const RUSTDOC_2021_12_05_CSS: &str =
    include_str!(concat!(env!("OUT_DIR"), "/rustdoc-2021-12-05.css"));
const STATIC_SEARCH_PATHS: &[&str] = &["static", "vendor"];

pub(crate) async fn static_handler(Path(path): Path<String>) -> AxumResult<impl IntoResponse> {
    let text_css: Mime = "text/css".parse().unwrap();

    Ok(match path.as_str() {
        "vendored.css" => build_response(VENDORED_CSS, text_css),
        "style.css" => build_response(STYLE_CSS, text_css),
        "rustdoc.css" => build_response(RUSTDOC_CSS, text_css),
        "rustdoc-2021-12-05.css" => build_response(RUSTDOC_2021_12_05_CSS, text_css),
        file => match serve_file(file).await {
            Ok(response) => response.into_response(),
            Err(err) => return Err(err),
        },
    })
}

async fn serve_file(file: &str) -> AxumResult<impl IntoResponse> {
    // Find the first path that actually exists
    let path = STATIC_SEARCH_PATHS
        .iter()
        .find_map(|root| {
            let path = path::Path::new(root).join(file);
            if !path.exists() {
                return None;
            }

            // Prevent accessing static files outside the root. This could happen if the path
            // contains `/` or `..`. The check doesn't outright prevent those strings to be present
            // to allow accessing files in subdirectories.
            let canonical_path = std::fs::canonicalize(path).ok()?;
            let canonical_root = std::fs::canonicalize(root).ok()?;
            if canonical_path.starts_with(canonical_root) {
                Some(canonical_path)
            } else {
                None
            }
        })
        .ok_or(AxumNope::ResourceNotFound)?;

    let contents = fs::read(&path)
        .await
        .with_context(|| format!("failed to read static file {}", path.display()))
        .map_err(|e| {
            report_error(&e);
            AxumNope::InternalServerError
        })?;

    // If we can detect the file's mime type, set it
    // MimeGuess misses a lot of the file types we need, so there's a small wrapper
    // around it
    let content_type: Mime = if file == "opensearch.xml" {
        "application/opensearchdescription+xml".parse().unwrap()
    } else {
        path.extension()
            .and_then(OsStr::to_str)
            .and_then(|ext| match ext {
                "eot" => Some("application/vnd.ms-fontobject".parse().unwrap()),
                "woff2" => Some("application/font-woff2".parse().unwrap()),
                "ttf" => Some("application/x-font-ttf".parse().unwrap()),
                _ => MimeGuess::from_path(&path).first(),
            })
            .unwrap_or(mime::APPLICATION_OCTET_STREAM)
    };

    Ok(build_response(contents, content_type))
}

fn build_response<R>(resource: R, content_type: Mime) -> Response
where
    R: AsRef<[u8]>,
{
    (
        StatusCode::OK,
        Extension(CachePolicy::ForeverInCdnAndBrowser),
        [
            (CONTENT_LENGTH, resource.as_ref().len().to_string()),
            (CONTENT_TYPE, content_type.to_string()),
            (LAST_MODIFIED, fmt_http_date(SystemTime::from(Utc::now()))),
        ],
        resource.as_ref().to_vec(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{serve_file, STATIC_SEARCH_PATHS, STYLE_CSS, VENDORED_CSS};
    use crate::{
        test::{assert_cache_control, wrapper},
        web::{cache::CachePolicy, error::AxumNope},
    };
    use reqwest::StatusCode;
    use std::fs;
    use test_case::test_case;

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
            assert_eq!(resp.text()?, STYLE_CSS);

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

    #[test_case("/-/static/index.js", "copyTextHandler")]
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
                Some(&"application/javascript".parse().unwrap()),
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

                    assert!(resp.status().is_success(), "failed to fetch {:?}", url);
                    assert_cache_control(&resp, CachePolicy::ForeverInCdnAndBrowser, &env.config());
                    assert_eq!(
                        resp.bytes()?,
                        fs::read(path).unwrap(),
                        "failed to fetch {:?}",
                        url,
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
                let url = format!("/-/static/{}", file);
                let resp = web.get(&url).send()?;

                assert_eq!(
                    resp.headers().get("Content-Type"),
                    Some(&mime.parse().unwrap()),
                    "{:?} has an incorrect content type",
                    url,
                );
            }

            Ok(())
        });
    }

    #[tokio::test]
    async fn directory_traversal() {
        const PATHS: &[&str] = &[
            "../LICENSE",
            "%2e%2e%2fLICENSE",
            "%2e%2e/LICENSE",
            "..%2fLICENSE",
            "%2e%2e%5cLICENSE",
        ];

        for path in PATHS {
            // This doesn't test an actual web request as the web framework used at the time of
            // writing this test (iron 0.5) already resolves `..` before calling any handler.
            //
            // Still, the test ensures the underlying function called by the request handler to
            // serve the file also includes protection for path traversal, in the event we switch
            // to a framework that doesn't include builtin protection in the future.
            assert!(
                matches!(serve_file(path).await, Err(AxumNope::ResourceNotFound)),
                "{} did not return a 404",
                path
            );
        }
    }
}
