use crate::{
    cache::CachePolicy, cache::STATIC_ASSET_CACHE_POLICY, metrics::request_recorder,
    routes::get_static,
};
use anyhow::{Result, bail};
use axum::{
    Router as AxumRouter,
    extract::{Extension, Request},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get_service,
};
use axum_extra::{
    headers::{ContentType, ETag, HeaderMapExt as _},
    typed_header::TypedHeader,
};
use docs_rs_headers::IfNoneMatch;
use docs_rs_mimes::APPLICATION_OPENSEARCH_XML;
use http::{StatusCode, Uri};
use std::{
    env,
    path::{Path, PathBuf},
};
use tower_http::services::ServeDir;

const MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");
const STATIC_DIR_NAME: &str = "static";
const VENDOR_DIR_NAME: &str = "vendor";
const STATIC_DIR_NAMES: &[&str] = &[STATIC_DIR_NAME, VENDOR_DIR_NAME];

/// Find the root directory for serving our static assets.
///
/// We have two directories we expect: `vendor` and `static`.
///
/// First we check if they exist in the current working directory:
/// this works
/// * inside the docker container, or
/// * any production deploy,
/// * when running `cargo run` from inside the `docs_rs_web` subcrate.
///
/// If they don't exist there, we try to find the folders in
/// `CARGO_MANIFEST_DIR`.
/// This allows running the server from the project root.
pub(crate) fn static_root_dir() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(MANIFEST_DIR);
    for candidate in [env::current_dir()?, manifest_dir] {
        if STATIC_DIR_NAMES
            .iter()
            .all(|name| candidate.join(name).is_dir())
        {
            return Ok(candidate);
        }
    }

    bail!(
        "Could not find static root directory containing '{STATIC_DIR_NAME}' and '{VENDOR_DIR_NAME}' folders"
    );
}

const VENDORED_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/vendored.css"));
const STYLE_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/style.css"));
const RUSTDOC_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/rustdoc.css"));
const RUSTDOC_2021_12_05_CSS: &str =
    include_str!(concat!(env!("OUT_DIR"), "/rustdoc-2021-12-05.css"));
const RUSTDOC_2025_08_20_CSS: &str =
    include_str!(concat!(env!("OUT_DIR"), "/rustdoc-2025-08-20.css"));

include!(concat!(env!("OUT_DIR"), "/static_etag_map.rs"));

fn build_static_css_response(content: &'static str) -> impl IntoResponse {
    (
        Extension(STATIC_ASSET_CACHE_POLICY),
        TypedHeader(ContentType::from(mime::TEXT_CSS)),
        content,
    )
}

async fn set_needed_static_headers(req: Request, next: Next) -> Response {
    let req_path = req.uri().path();
    let is_opensearch_xml = req_path.ends_with("/opensearch.xml");

    let mut response = next.run(req).await;

    if response.status().is_success() {
        response.extensions_mut().insert(STATIC_ASSET_CACHE_POLICY);
    }

    if is_opensearch_xml {
        // overwrite the content type for opensearch.xml,
        // otherwise mime-guess would return `text/xml`.
        response
            .headers_mut()
            .typed_insert(ContentType::from(APPLICATION_OPENSEARCH_XML.clone()));
    }

    response
}

async fn conditional_get(
    partial_uri: Uri,
    if_none_match: Option<TypedHeader<IfNoneMatch>>,
    req: Request,
    next: Next,
) -> Response {
    debug_assert!(STATIC_ETAG_MAP.is_sorted());

    let if_none_match = if_none_match.map(|th| th.0);
    let resource_path = partial_uri.path().trim_start_matches('/');
    let Some(etag) = STATIC_ETAG_MAP
        .binary_search_by_key(&resource_path, |(path, _)| *path)
        .ok()
        .map(|pos| {
            let etag = STATIC_ETAG_MAP[pos].1;
            etag.parse::<ETag>()
                .expect("compile time generated, should always pass")
        })
    else {
        let res = next.run(req).await;

        debug_assert!(
            !res.status().is_success(),
            "no etag found for static resource at {}, but should exist.\n{:?}",
            resource_path,
            STATIC_ETAG_MAP,
        );

        return res;
    };

    if let Some(if_none_match) = if_none_match
        && !if_none_match.precondition_passes(&etag)
    {
        return (
            StatusCode::NOT_MODIFIED,
            TypedHeader(etag),
            Extension(CachePolicy::ForeverInCdnAndBrowser),
        )
            .into_response();
    }

    let mut res = next.run(req).await;
    let status = res.status();
    // Typically we only end up here when we have a successful response.
    //
    // But there is an edge case, that only happens when there is a path
    // where we were able to statically generate an ETag in the
    // build-script, but the file can't be found later.
    // Until now, happend only in local dev, but would also happen
    // if the static file was deleted from the server after deployment.
    if status.is_success() {
        res.headers_mut().typed_insert(etag);
    }
    res
}

pub(crate) fn build_static_router(root: impl AsRef<Path>) -> AxumRouter {
    let root = root.as_ref();
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
            get_service(
                ServeDir::new(root.join(STATIC_DIR_NAME))
                    .fallback(ServeDir::new(root.join(VENDOR_DIR_NAME))),
            )
            .layer(middleware::from_fn(set_needed_static_headers))
            .layer(middleware::from_fn(|request, next| async {
                request_recorder(request, next, Some("static resource")).await
            })),
        )
        .layer(middleware::from_fn(conditional_get))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        handlers::apply_middleware,
        page::TemplateData,
        testing::{
            AxumResponseTestExt, AxumRouterTestExt, TestEnvironment, TestEnvironmentExt as _,
            async_wrapper,
        },
    };
    use axum::{Router, body::Body};
    use docs_rs_headers::compute_etag;
    use http::{
        HeaderMap,
        header::{CONTENT_LENGTH, CONTENT_TYPE, ETAG},
    };
    use std::{fs, sync::Arc};
    use test_case::test_case;
    use tower::ServiceExt as _;

    fn content_length(resp: &Response) -> u64 {
        resp.headers()
            .get(CONTENT_LENGTH)
            .expect("content-length header")
            .to_str()
            .unwrap()
            .parse()
            .unwrap()
    }

    fn etag(resp: &Response) -> ETag {
        resp.headers().typed_get().unwrap()
    }

    async fn test_conditional_get(web: &Router, path: &str) -> anyhow::Result<()> {
        fn req(path: &str, f: impl FnOnce(&mut HeaderMap)) -> Request {
            let mut builder = Request::builder().uri(path);
            f(builder.headers_mut().unwrap());
            builder.body(Body::empty()).unwrap()
        }

        // original request = 200
        let resp = web.clone().oneshot(req(path, |_| {})).await?;

        assert_eq!(resp.status(), StatusCode::OK);
        let etag = etag(&resp);

        {
            // if-none-match with correct etag
            let if_none_match: IfNoneMatch = etag.into();

            let cached_response = web
                .clone()
                .oneshot(req(path, |h| h.typed_insert(if_none_match)))
                .await?;

            assert_eq!(cached_response.status(), StatusCode::NOT_MODIFIED);
        }

        {
            let other_if_none_match: IfNoneMatch = "\"some-other-etag\""
                .parse::<ETag>()
                .expect("valid etag")
                .into();

            let uncached_response = web
                .clone()
                .oneshot(req(path, |h| h.typed_insert(other_if_none_match)))
                .await?;

            assert_eq!(uncached_response.status(), StatusCode::OK);
        }

        Ok(())
    }

    #[test]
    fn style_css() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            const PATH: &str = "/-/static/style.css";
            let resp = web.get(PATH).await?;
            assert!(resp.status().is_success());
            resp.assert_cache_control(CachePolicy::ForeverInCdnAndBrowser, env.config());
            let headers = resp.headers();
            assert_eq!(
                headers.get(CONTENT_TYPE),
                Some(&"text/css".parse().unwrap()),
            );

            assert_eq!(content_length(&resp), STYLE_CSS.len() as u64);
            assert_eq!(etag(&resp), compute_etag(STYLE_CSS.as_bytes()));
            assert_eq!(resp.bytes().await?, STYLE_CSS.as_bytes());

            test_conditional_get(&web, PATH).await?;

            Ok(())
        });
    }

    #[test]
    fn vendored_css() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            const PATH: &str = "/-/static/vendored.css";

            let resp = web.get(PATH).await?;
            assert!(resp.status().is_success(), "{}", resp.text().await?);

            resp.assert_cache_control(CachePolicy::ForeverInCdnAndBrowser, env.config());
            assert_eq!(
                resp.headers().get(CONTENT_TYPE),
                Some(&"text/css".parse().unwrap()),
            );
            assert_eq!(content_length(&resp), VENDORED_CSS.len() as u64);
            assert_eq!(etag(&resp), compute_etag(VENDORED_CSS.as_bytes()));
            assert_eq!(resp.text().await?, VENDORED_CSS);

            test_conditional_get(&web, PATH).await?;

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
            assert!(resp.headers().get(ETAG).is_none());

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
            resp.assert_cache_control(CachePolicy::ForeverInCdnAndBrowser, env.config());
            assert_eq!(
                resp.headers().get(CONTENT_TYPE),
                Some(&"text/javascript".parse().unwrap()),
            );
            assert!(content_length(&resp) > 10);
            etag(&resp); // panics if etag missing or invalid
            assert!(resp.text().await?.contains(expected_content));

            test_conditional_get(&web, path).await?;

            Ok(())
        });
    }

    #[test]
    fn static_files() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;

            for root in STATIC_DIR_NAMES {
                let root = static_root_dir()?.join(root);
                for entry in walkdir::WalkDir::new(&root) {
                    let entry = entry?;
                    if !entry.file_type().is_file() {
                        continue;
                    }
                    let file = entry.path().strip_prefix(&root).unwrap();
                    let path = entry.path();

                    let url = format!("/-/static/{}", file.to_str().unwrap());
                    let resp = web.get(&url).await?;

                    assert!(resp.status().is_success(), "failed to fetch {url:?}");
                    resp.assert_cache_control(CachePolicy::ForeverInCdnAndBrowser, env.config());
                    let content = fs::read(path).unwrap();
                    assert_eq!(etag(&resp), compute_etag(&content));
                    assert_eq!(resp.bytes().await?, content, "failed to fetch {url:?}",);

                    test_conditional_get(&web, &url).await?;
                }
            }

            Ok(())
        });
    }

    #[test]
    fn static_file_that_doesnt_exist() {
        async_wrapper(|env| async move {
            let response = env.web_app().await.get("/-/static/whoop-de-do.png").await?;
            response.assert_cache_control(CachePolicy::NoCaching, env.config());
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert!(response.headers().get(ETAG).is_none());

            Ok(())
        });
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn static_files_should_exist_but_is_locally_deleted() -> Result<()> {
        let env = TestEnvironment::new().await?;

        /// build a small axum app with middleware, but just with the static router only.
        async fn build_static_app(env: &TestEnvironment, root: impl AsRef<Path>) -> Result<Router> {
            let template_data = Arc::new(TemplateData::new(1).unwrap());
            apply_middleware(
                build_static_router(root),
                env.config().clone(),
                env.context().clone(),
                Some(template_data),
            )
            .await
        }

        const PATH: &str = "/menu.js";
        {
            // Sanity check if we have a path that should exist
            let web = env.web_app().await;
            web.assert_success(&format!("/-/static{PATH}")).await?;

            // and if our static router thing works theoretically
            let static_app = build_static_app(&env, &static_root_dir()?).await?;
            static_app.assert_success(PATH).await?;
        }

        // set up a broken static router.
        // The compile-time generated etag map says `menu.js` should exist,
        // but in the given root for static files, it's missing.
        let tempdir = tempfile::tempdir()?;
        let static_app = build_static_app(&env, &tempdir).await?;

        // before bugfix, this would add caching headers, and
        // trigger a `debug_assert`.
        // The 404 is what we expect.
        // `assert_not_found` also asserts if no-caching headers are set.
        static_app.assert_not_found(PATH).await?;

        Ok(())
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
                    resp.headers().get(CONTENT_TYPE),
                    Some(&mime.parse().unwrap()),
                    "{url:?} has an incorrect content type",
                );
            }

            Ok(())
        });
    }
}
