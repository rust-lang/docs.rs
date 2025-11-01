use crate::web::{
    cache::CachePolicy, error::AxumNope, metrics::request_recorder, statics::build_static_router,
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

pub(super) fn build_metric_routes() -> AxumRouter {
    AxumRouter::new()
        .route_with_tsr(
            "/about/metrics/instance",
            get_internal(super::metrics::instance_metrics_handler),
        )
        .route_with_tsr(
            "/about/metrics/service",
            get_internal(super::metrics::service_metrics_handler),
        )
        .route_with_tsr(
            "/about/metrics",
            get_internal(super::metrics::metrics_handler),
        )
}

fn cached_permanent_redirect(uri: &str) -> impl IntoResponse {
    (
        Extension(CachePolicy::ForeverInCdnAndBrowser),
        Redirect::permanent(uri),
    )
}

pub(super) fn build_axum_routes() -> AxumRouter {
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
        .route_with_tsr(
            "/sitemap.xml",
            get_internal(super::sitemap::sitemapindex_handler),
        )
        .route_with_tsr(
            "/-/sitemap/{letter}/sitemap.xml",
            get_internal(super::sitemap::sitemap_handler),
        )
        .route_with_tsr(
            "/about/builds",
            get_internal(super::sitemap::about_builds_handler),
        )
        .merge(build_metric_routes())
        .route_with_tsr("/about", get_internal(super::sitemap::about_handler))
        .route_with_tsr(
            "/about/{subpage}",
            get_internal(super::sitemap::about_handler),
        )
        .route("/", get_internal(super::releases::home_page))
        .route_with_tsr(
            "/releases",
            get_internal(super::releases::recent_releases_handler),
        )
        .route_with_tsr(
            "/releases/recent/{page}",
            get_internal(super::releases::recent_releases_handler),
        )
        .route_with_tsr(
            "/releases/stars",
            get_internal(super::releases::releases_by_stars_handler),
        )
        .route_with_tsr(
            "/releases/stars/{page}",
            get_internal(super::releases::releases_by_stars_handler),
        )
        .route_with_tsr(
            "/releases/recent-failures",
            get_internal(super::releases::releases_recent_failures_handler),
        )
        .route_with_tsr(
            "/releases/recent-failures/{page}",
            get_internal(super::releases::releases_recent_failures_handler),
        )
        .route_with_tsr(
            "/releases/failures",
            get_internal(super::releases::releases_failures_by_stars_handler),
        )
        .route_with_tsr(
            "/releases/failures/{page}",
            get_internal(super::releases::releases_failures_by_stars_handler),
        )
        .route_with_tsr(
            "/crate/{name}",
            get_internal(super::crate_details::crate_details_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}",
            get_internal(super::crate_details::crate_details_handler),
        )
        .route_with_tsr(
            "/releases/feed",
            get_internal(super::releases::releases_feed_handler),
        )
        .route_with_tsr(
            "/releases/{owner}",
            get_internal(super::releases::owner_handler),
        )
        .route_with_tsr(
            "/releases/{owner}/{page}",
            get_internal(super::releases::owner_handler),
        )
        .route_with_tsr(
            "/releases/activity",
            get_internal(super::releases::activity_handler),
        )
        .route_with_tsr(
            "/releases/search",
            get_internal(super::releases::search_handler),
        )
        .route_with_tsr(
            "/releases/queue",
            get_internal(super::releases::build_queue_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/builds",
            get_internal(super::builds::build_list_handler),
        )
        .route(
            "/crate/{name}/{version}/rebuild",
            post_internal(super::builds::build_trigger_rebuild_handler),
        )
        .route(
            "/crate/{name}/{version}/status.json",
            get_internal(super::status::status_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/builds/{id}",
            get_internal(super::build_details::build_details_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/builds/{id}/{filename}",
            get_internal(super::build_details::build_details_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/features",
            get_internal(super::features::build_features_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/source/",
            get_internal(super::source::source_browser_handler),
        )
        .route(
            "/crate/{name}/{version}/source/{*path}",
            get_internal(super::source::source_browser_handler),
        )
        .route(
            "/crate/{name}/{version}/menus/platforms/{target}/",
            get_internal(super::crate_details::get_all_platforms),
        )
        .route(
            "/crate/{name}/{version}/menus/platforms/{target}/{*path}",
            get_internal(super::crate_details::get_all_platforms),
        )
        .route(
            "/crate/{name}/{version}/menus/platforms/",
            get_internal(super::crate_details::get_all_platforms_root),
        )
        .route(
            "/crate/{name}/{version}/menus/releases/{*path}",
            get_internal(super::crate_details::get_all_releases),
        )
        .route(
            "/-/rustdoc.static/{*path}",
            get_internal(super::rustdoc::static_asset_handler),
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
            get_internal(super::rustdoc::download_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/json.gz",
            get_internal(super::rustdoc::json_download_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/json.zst",
            get_internal(super::rustdoc::json_download_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/json",
            get_internal(super::rustdoc::json_download_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/json/{format_version}",
            get_internal(super::rustdoc::json_download_handler),
        )
        .route(
            "/crate/{name}/{version}/target-redirect/{*path}",
            get_internal(super::rustdoc::target_redirect_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/{target}/json.gz",
            get_internal(super::rustdoc::json_download_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/{target}/json.zst",
            get_internal(super::rustdoc::json_download_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/{target}/json",
            get_internal(super::rustdoc::json_download_handler),
        )
        .route_with_tsr(
            "/crate/{name}/{version}/{target}/json/{format_version}",
            get_internal(super::rustdoc::json_download_handler),
        )
        .route(
            "/{name}/badge.svg",
            get_internal(super::rustdoc::badge_handler),
        )
        .route(
            "/{name}",
            get_rustdoc(super::rustdoc::rustdoc_redirector_handler),
        )
        .route(
            "/{name}/",
            get_rustdoc(super::rustdoc::rustdoc_redirector_handler),
        )
        .route(
            "/{name}/{version}",
            get_rustdoc(super::rustdoc::rustdoc_redirector_handler),
        )
        .route(
            "/{name}/{version}/",
            get_rustdoc(super::rustdoc::rustdoc_redirector_handler),
        )
        .route(
            "/{name}/{version}/all.html",
            get_rustdoc(super::rustdoc::rustdoc_html_server_handler),
        )
        .route(
            "/{name}/{version}/help.html",
            get_rustdoc(super::rustdoc::rustdoc_html_server_handler),
        )
        .route(
            "/{name}/{version}/settings.html",
            get_rustdoc(super::rustdoc::rustdoc_html_server_handler),
        )
        .route(
            "/{name}/{version}/scrape-examples-help.html",
            get_rustdoc(super::rustdoc::rustdoc_html_server_handler),
        )
        .route(
            "/{name}/{version}/{target}",
            get_rustdoc(super::rustdoc::rustdoc_redirector_handler),
        )
        .route(
            "/{name}/{version}/{target}/",
            get_rustdoc(super::rustdoc::rustdoc_html_server_handler),
        )
        .route(
            "/{name}/{version}/{target}/{*path}",
            get_rustdoc(super::rustdoc::rustdoc_html_server_handler),
        )
        .fallback(fallback)
}

async fn fallback() -> impl IntoResponse {
    AxumNope::ResourceNotFound
}

#[cfg(test)]
mod tests {
    use crate::test::{AxumResponseTestExt, AxumRouterTestExt, async_wrapper};
    use crate::web::cache::CachePolicy;
    use reqwest::StatusCode;

    #[test]
    fn test_root_redirects() {
        async_wrapper(|env| async move {
            let web = env.web_app().await;
            let config = env.config();
            // These are "well-known" resources that will be requested from the root, but support
            // redirection
            web.assert_redirect_cached(
                "/favicon.ico",
                "/-/static/favicon.ico",
                CachePolicy::ForeverInCdnAndBrowser,
                config,
            )
            .await?;
            web.assert_redirect_cached(
                "/robots.txt",
                "/-/static/robots.txt",
                CachePolicy::ForeverInCdnAndBrowser,
                config,
            )
            .await?;

            // This has previously been served with a url pointing to the root, it may be
            // plausible to remove the redirects in the future, but for now we need to keep serving
            // it.
            web.assert_redirect_cached(
                "/opensearch.xml",
                "/-/static/opensearch.xml",
                CachePolicy::ForeverInCdnAndBrowser,
                config,
            )
            .await?;

            Ok(())
        });
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
            let storage = env.async_storage();
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
