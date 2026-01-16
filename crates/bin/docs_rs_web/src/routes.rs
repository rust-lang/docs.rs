use crate::{
    cache::CachePolicy,
    error::AxumNope,
    handlers::{
        about, build_details, builds, crate_details, features, releases, rustdoc, sitemap, source,
        statics::build_static_router, status,
    },
    metrics::request_recorder,
};
use askama::Template;
use axum::{
    Extension, Router as AxumRouter,
    extract::Request as AxumHttpRequest,
    handler::Handler as AxumHandler,
    middleware::{self, Next},
    response::{IntoResponse, Redirect},
    routing::{MethodRouter, get, post},
};
use axum_extra::routing::RouterExt;
use std::convert::Infallible;
use tracing::{debug, instrument};

const INTERNAL_PREFIXES: &[&str] = &["-", "about", "crate", "releases", "sitemap.xml"];

#[instrument(skip_all)]
pub(crate) fn get_static<H, T, S>(handler: H) -> MethodRouter<S, Infallible>
where
    H: AxumHandler<T, S>,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    get(handler).route_layer(middleware::from_fn(|request, next| async {
        request_recorder(request, next, Some("static resource")).await
    }))
}

#[instrument(skip_all)]
fn get_internal<H, T, S>(handler: H) -> MethodRouter<S, Infallible>
where
    H: AxumHandler<T, S>,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    get(handler).route_layer(middleware::from_fn(|request, next| async {
        request_recorder(request, next, None).await
    }))
}

#[instrument(skip_all)]
fn post_internal<H, T, S>(handler: H) -> MethodRouter<S, Infallible>
where
    H: AxumHandler<T, S>,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    post(handler).route_layer(middleware::from_fn(|request, next| async {
        request_recorder(request, next, None).await
    }))
}

#[instrument(skip_all)]
fn get_rustdoc<H, T, S>(handler: H) -> MethodRouter<S, Infallible>
where
    H: AxumHandler<T, S>,
    T: 'static,
    S: Clone + Send + Sync + 'static,
{
    get(handler)
        .route_layer(middleware::from_fn(|request, next| async {
            request_recorder(request, next, Some("rustdoc page")).await
        }))
        .layer(middleware::from_fn(block_blacklisted_prefixes_middleware))
}

async fn block_blacklisted_prefixes_middleware(
    request: AxumHttpRequest,
    next: Next,
) -> impl IntoResponse {
    if let Some(first_component) = request.uri().path().trim_matches('/').split('/').next()
        && !first_component.is_empty()
        && (INTERNAL_PREFIXES.binary_search(&first_component).is_ok())
    {
        debug!(
            first_component = first_component,
            uri = ?request.uri(),
            "blocking blacklisted prefix"
        );
        return AxumNope::CrateNotFound.into_response();
    }

    next.run(request).await
}

fn cached_permanent_redirect(uri: &str) -> impl IntoResponse {
    (
        Extension(CachePolicy::ForeverInCdnAndBrowser),
        Redirect::permanent(uri),
    )
}

pub(crate) fn build_axum_routes() -> AxumRouter {
    // hint for naming axum routes:
    // when routes overlap, the route parameters at the same position
    // have to use the same name:
    //
    // These routes work together:
    // - `/{name}/{version}/settings.html`
    // - `/{name}/{version}/{target}`
    // and axum can prioritize the more specific route.
    //
    // This panics because of conflicting routes:
    // - `/{name}/{version}/settings.html`
    // - `/{crate}/{version}/{target}`
    //
    AxumRouter::new()
        // Well known resources, robots.txt and favicon.ico support redirection, the sitemap.xml
        // must live at the site root:
        //   https://developers.google.com/search/reference/robots_txt#handling-http-result-codes
        //   https://support.google.com/webmasters/answer/183668?hl=en
        .route(
            "/robots.txt",
            get_static(|| async { cached_permanent_redirect("/-/static/robots.txt") }),
        )
        .route(
            "/favicon.ico",
            get_static(|| async { cached_permanent_redirect("/-/static/favicon.ico") }),
        )
        // `.nest` with fallbacks is currently broken, `.nest_service works
        // https://github.com/tokio-rs/axum/issues/3138
        .nest_service("/-/static", build_static_router())
        .route(
            "/opensearch.xml",
            get_static(|| async { cached_permanent_redirect("/-/static/opensearch.xml") }),
        )
        .route_with_tsr("/sitemap.xml", get_internal(sitemap::sitemapindex_handler))
        .route_with_tsr(
            "/-/sitemap/{letter}/sitemap.xml",
            get_internal(sitemap::sitemap_handler),
        )
        .route_with_tsr("/about/builds", get_internal(about::about_builds_handler))
        .route_with_tsr("/about", get_internal(about::about_handler))
        .route_with_tsr("/about/{subpage}", get_internal(about::about_handler))
        .route("/", get_internal(releases::home_page))
        .route_with_tsr("/releases", get_internal(releases::recent_releases_handler))
        .route_with_tsr(
            "/releases/recent/{page}",
            get_internal(releases::recent_releases_handler),
        )
        .route_with_tsr(
            "/releases/stars",
            get_internal(releases::releases_by_stars_handler),
        )
        .route_with_tsr(
            "/releases/stars/{page}",
            get_internal(releases::releases_by_stars_handler),
        )
        .route_with_tsr(
            "/releases/recent-failures",
            get_internal(releases::releases_recent_failures_handler),
        )
        .route_with_tsr(
            "/releases/recent-failures/{page}",
            get_internal(releases::releases_recent_failures_handler),
        )
        .route_with_tsr(
            "/releases/failures",
            get_internal(releases::releases_failures_by_stars_handler),
        )
        .route_with_tsr(
            "/releases/failures/{page}",
            get_internal(releases::releases_failures_by_stars_handler),
        )
        .route(
            "/crate/{name}",
            get_internal(crate_details::crate_details_handler),
        )
        .route(
            "/crate/{name}/",
            get_internal(crate_details::crate_details_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}",
            get_internal(crate_details::crate_details_handler),
        )
        .route_with_tsr(
            "/releases/feed",
            get_internal(releases::releases_feed_handler),
        )
        .route_with_tsr("/releases/{owner}", get_internal(releases::owner_handler))
        .route_with_tsr(
            "/releases/{owner}/{page}",
            get_internal(releases::owner_handler),
        )
        .route_with_tsr(
            "/releases/activity",
            get_internal(releases::activity_handler),
        )
        .route_with_tsr("/releases/search", get_internal(releases::search_handler))
        .route_with_tsr(
            "/releases/queue",
            get_internal(releases::build_queue_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/builds",
            get_internal(builds::build_list_handler),
        )
        .route(
            "/crate/{name}/{version}/rebuild",
            post_internal(builds::build_trigger_rebuild_handler),
        )
        .route(
            "/crate/{name}/{version}/status.json",
            get_internal(status::status_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/builds/{id}",
            get_internal(build_details::build_details_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/builds/{id}/{filename}",
            get_internal(build_details::build_details_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/features",
            get_internal(features::build_features_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/source/",
            get_internal(source::source_browser_handler),
        )
        .route(
            "/crate/{name}/{version}/source/{*path}",
            get_internal(source::source_browser_handler),
        )
        .route(
            "/crate/{name}/{version}/menus/platforms/{target}/",
            get_internal(crate_details::get_all_platforms),
        )
        .route(
            "/crate/{name}/{version}/menus/platforms/{target}/{*path}",
            get_internal(crate_details::get_all_platforms),
        )
        .route(
            "/crate/{name}/{version}/menus/platforms/",
            get_internal(crate_details::get_all_platforms_root),
        )
        .route(
            "/crate/{name}/{version}/menus/releases/{*path}",
            get_internal(crate_details::get_all_releases),
        )
        .route(
            "/-/rustdoc.static/{*path}",
            get_internal(rustdoc::static_asset_handler),
        )
        .route(
            "/-/storage-change-detection.html",
            get_internal(|| async {
                #[derive(Template)]
                #[template(path = "storage-change-detection.html")]
                #[derive(Debug, Clone)]
                struct StorageChangeDetection;
                crate::impl_axum_webpage!(
                    StorageChangeDetection,
                    cache_policy = |_| CachePolicy::ForeverInCdnAndBrowser,
                );
                StorageChangeDetection
            }),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/download",
            get_internal(rustdoc::download_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/json.gz",
            get_internal(rustdoc::json_download_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/json.zst",
            get_internal(rustdoc::json_download_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/json",
            get_internal(rustdoc::json_download_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/json/{format_version}",
            get_internal(rustdoc::json_download_handler),
        )
        .route(
            "/crate/{name}/{version}/target-redirect/{*path}",
            get_internal(rustdoc::target_redirect_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/{target}/json.gz",
            get_internal(rustdoc::json_download_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/{target}/json.zst",
            get_internal(rustdoc::json_download_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/{target}/json",
            get_internal(rustdoc::json_download_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/{target}/json/{format_version}",
            get_internal(rustdoc::json_download_handler),
        )
        .route("/{name}/badge.svg", get_internal(rustdoc::badge_handler))
        .route("/{name}", get_rustdoc(rustdoc::rustdoc_redirector_handler))
        .route("/{name}/", get_rustdoc(rustdoc::rustdoc_redirector_handler))
        .route(
            "/{name}/{version}",
            get_rustdoc(rustdoc::rustdoc_redirector_handler),
        )
        .route(
            "/{name}/{version}/",
            get_rustdoc(rustdoc::rustdoc_redirector_handler),
        )
        .route(
            "/{name}/{version}/all.html",
            get_rustdoc(rustdoc::rustdoc_html_server_handler),
        )
        .route(
            "/{name}/{version}/help.html",
            get_rustdoc(rustdoc::rustdoc_html_server_handler),
        )
        .route(
            "/{name}/{version}/settings.html",
            get_rustdoc(rustdoc::rustdoc_html_server_handler),
        )
        .route(
            "/{name}/{version}/scrape-examples-help.html",
            get_rustdoc(rustdoc::rustdoc_html_server_handler),
        )
        .route(
            "/{name}/{version}/{target}",
            get_rustdoc(rustdoc::rustdoc_redirector_handler),
        )
        .route(
            "/{name}/{version}/{target}/",
            get_rustdoc(rustdoc::rustdoc_html_server_handler),
        )
        .route(
            "/{name}/{version}/{target}/{*path}",
            get_rustdoc(rustdoc::rustdoc_html_server_handler),
        )
        .fallback(fallback)
}

async fn fallback() -> impl IntoResponse {
    AxumNope::ResourceNotFound
}

#[cfg(test)]
mod tests {
    use crate::cache::CachePolicy;
    use crate::testing::{
        AxumResponseTestExt, AxumRouterTestExt, TestEnvironment, TestEnvironmentExt as _,
        async_wrapper,
    };
    use anyhow::Result;
    use reqwest::StatusCode;
    use test_case::test_case;

    // These are "well-known" resources that will be requested from the root, but support
    // redirection
    #[test_case("/favicon.ico", "/-/static/favicon.ico")]
    #[test_case("/robots.txt", "/-/static/robots.txt")]
    // This has previously been served with a url pointing to the root, it may be
    // plausible to remove the redirects in the future, but for now we need to keep serving
    // it.
    #[test_case("/opensearch.xml", "/-/static/opensearch.xml")]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_root_redirects(path: &str, target: &str) -> Result<()> {
        let env = TestEnvironment::new().await?;
        let web = env.web_app().await;
        let config = env.config();

        web.assert_redirect_cached(path, target, CachePolicy::ForeverInCdnAndBrowser, config)
            .await?;

        Ok(())
    }

    #[test]
    fn serve_rustdoc_content_not_found() {
        async_wrapper(|env| async move {
            let response = env
                .web_app()
                .await
                .get("/-/rustdoc.static/style.css")
                .await?;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            response.assert_cache_control(CachePolicy::NoCaching, env.config());
            Ok(())
        })
    }

    #[test]
    fn serve_rustdoc_content() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;
            let storage = env.storage()?;
            storage
                .store_one("/rustdoc-static/style.css", "content".as_bytes())
                .await?;
            storage
                .store_one("/will_not/be_found.css", "something".as_bytes())
                .await?;

            let response = web.get("/-/rustdoc.static/style.css").await?;
            assert!(response.status().is_success());
            response.assert_cache_control(CachePolicy::ForeverInCdnAndBrowser, env.config());
            assert_eq!(response.text().await?, "content");

            assert_eq!(
                web.get("/-/rustdoc.static/will_not/be_found.css")
                    .await?
                    .status(),
                StatusCode::NOT_FOUND
            );
            Ok(())
        })
    }
}
