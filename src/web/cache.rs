use crate::{
    config::Config,
    db::types::krate_name::KrateName,
    web::{
        extractors::Path,
        headers::{SURROGATE_CONTROL, SURROGATE_KEY, SurrogateKeys},
    },
};
use axum::{
    Extension,
    extract::{MatchedPath, Request as AxumHttpRequest},
    middleware::Next,
    response::Response as AxumResponse,
};
use axum_extra::headers::HeaderMapExt as _;
use http::{
    HeaderMap, HeaderValue, StatusCode,
    header::{CACHE_CONTROL, ETAG},
};
use serde::Deserialize;
use std::sync::Arc;
use tracing::error;

#[derive(Debug, Clone, PartialEq)]
pub struct ResponseCacheHeaders {
    pub cache_control: Option<HeaderValue>,
    pub surrogate_control: Option<HeaderValue>,
    pub needs_cdn_invalidation: bool,
}

impl ResponseCacheHeaders {
    fn set_on_response(&self, headers: &mut HeaderMap) {
        if let Some(ref cache_control) = self.cache_control {
            headers.insert(CACHE_CONTROL, cache_control.clone());
        }
        if let Some(ref surrogate_control) = self.surrogate_control {
            headers.insert(&SURROGATE_CONTROL, surrogate_control.clone());
        }
    }
}

/// No caching in the CDN & in the browser.
/// Browser & CDN often still store the file,
/// but then always revalidate using `If-Modified-Since` (with last modified)
/// or `If-None-Match` (with etag).
/// Browser might still sometimes use cached content, for example when using
/// the "back" button.
pub static NO_CACHING: ResponseCacheHeaders = ResponseCacheHeaders {
    cache_control: Some(HeaderValue::from_static("max-age=0")),
    surrogate_control: None,
    needs_cdn_invalidation: false,
};

/// Cache for a short time in the browser & in the CDN.
/// Helps protecting against traffic spikes.
pub static SHORT: ResponseCacheHeaders = ResponseCacheHeaders {
    cache_control: Some(HeaderValue::from_static("public, max-age=60")),
    surrogate_control: None,
    needs_cdn_invalidation: false,
};

/// don't cache, don't even store. Never. Ever.
pub static NO_STORE_MUST_REVALIDATE: ResponseCacheHeaders = ResponseCacheHeaders {
    cache_control: Some(HeaderValue::from_static(
        "no-cache, no-store, must-revalidate, max-age=0",
    )),
    surrogate_control: None,
    needs_cdn_invalidation: false,
};

pub static FOREVER_IN_FASTLY_CDN: ResponseCacheHeaders = ResponseCacheHeaders {
    // explicitly forbid browser caching, same as NO_CACHING above.
    cache_control: Some(HeaderValue::from_static("max-age=0")),

    // set `surrogate-control`, cache forever in the CDN
    // https://www.fastly.com/documentation/reference/http/http-headers/Surrogate-Control/
    //
    // TODO: evaluate if we can / should set `stale-while-revalidate` or `stale-if-error` here,
    // especially in combination with our fastly compute service.
    // https://www.fastly.com/documentation/guides/concepts/edge-state/cache/stale/
    surrogate_control: Some(HeaderValue::from_static("max-age=31536000")),

    needs_cdn_invalidation: true,
};

/// cache forever in browser & CDN.
/// Only usable for content with unique filenames.
///
/// We use this policy mostly for static files, rustdoc toolchain assets,
/// or build assets.
pub static FOREVER_IN_CDN_AND_BROWSER: ResponseCacheHeaders = ResponseCacheHeaders {
    cache_control: Some(HeaderValue::from_static(
        "public, max-age=31104000, immutable",
    )),
    surrogate_control: None,
    needs_cdn_invalidation: false,
};

/// defines the wanted caching behaviour for a web response.
#[derive(Debug, Clone)]
#[cfg_attr(test, derive(strum::EnumIter))]
pub enum CachePolicy {
    /// no browser or CDN caching.
    /// In some cases the browser might still use cached content,
    /// for example when using the "back" button or when it can't
    /// connect to the server.
    NoCaching,
    /// don't cache, plus
    /// * enforce revalidation
    /// * never store
    NoStoreMustRevalidate,
    /// cache for a short time in the browser & CDN.
    /// right now: one minute.
    /// Can be used when the content can be a _little_ outdated,
    /// while protecting against spikes in traffic.
    ShortInCdnAndBrowser,
    /// cache forever in browser & CDN.
    /// Valid when you have hashed / versioned filenames and every rebuild would
    /// change the filename.
    ForeverInCdnAndBrowser,
    /// cache forever in CDN, but not in the browser.
    /// Since we control the CDN we can actively purge content that is cached like
    /// this, for example after building a crate.
    /// Example usage: `/latest/` rustdoc pages and their redirects.
    ForeverInCdn,
    /// cache forever in the CDN, but allow stale content in the browser.
    /// Example: rustdoc pages with the version in their URL.
    /// A browser will show the stale content while getting the up-to-date
    /// version from the origin server in the background.
    /// This helps building a PWA.
    ForeverInCdnAndStaleInBrowser,
}

impl CachePolicy {
    pub fn render(&self, config: &Config) -> ResponseCacheHeaders {
        match *self {
            CachePolicy::NoCaching => NO_CACHING.clone(),
            CachePolicy::NoStoreMustRevalidate => NO_STORE_MUST_REVALIDATE.clone(),
            CachePolicy::ShortInCdnAndBrowser => SHORT.clone(),
            CachePolicy::ForeverInCdnAndBrowser => FOREVER_IN_CDN_AND_BROWSER.clone(),
            CachePolicy::ForeverInCdn => {
                if config.cache_invalidatable_responses {
                    FOREVER_IN_FASTLY_CDN.clone()
                } else {
                    NO_CACHING.clone()
                }
            }
            CachePolicy::ForeverInCdnAndStaleInBrowser => {
                // when caching invalidatable responses is disabled, this results in NO_CACHING
                let mut forever_in_cdn = CachePolicy::ForeverInCdn.render(config);

                if config.cache_invalidatable_responses
                    && let Some(cache_control) =
                        config.cache_control_stale_while_revalidate.map(|seconds| {
                            format!("stale-while-revalidate={seconds}")
                                .parse::<HeaderValue>()
                                .unwrap()
                        })
                {
                    forever_in_cdn.cache_control = Some(cache_control);
                }

                forever_in_cdn
            }
        }
    }
}

/// All our routes use `{name}` to identify the crate name
/// in routes.
/// With this struct we can extract only that, if it exists.
#[derive(Deserialize)]
pub(crate) struct CrateParam {
    name: Option<String>,
}

pub(crate) async fn cache_middleware(
    Path(param): Path<CrateParam>,
    matched_route: Option<MatchedPath>,
    Extension(config): Extension<Arc<Config>>,
    req: AxumHttpRequest,
    next: Next,
) -> AxumResponse {
    let mut response = next.run(req).await;

    debug_assert!(
        !(response
            .headers()
            .keys()
            .any(|h| { h == CACHE_CONTROL || h == SURROGATE_CONTROL || h == SURROGATE_KEY })),
        "handlers should never set their own caching headers and only use CachePolicy to control caching. \n{:?}",
        response.headers(),
    );

    debug_assert!(
        response.status() == StatusCode::NOT_MODIFIED
            || response.status().is_success()
            || !response.headers().contains_key(ETAG),
        "only successful or not-modified responses should have etags. \n{:?}\n{:?}",
        response.status(),
        response.headers(),
    );

    // extract cache policy, default to "forbid caching everywhere".
    // We only use cache policies in our successful responses (with content, or redirect),
    // so any errors (4xx, 5xx) should always get "NoCaching".
    let cache_policy = response
        .extensions()
        .get::<CachePolicy>()
        .unwrap_or(&CachePolicy::NoCaching);
    let cache_headers = cache_policy.render(&config);
    let resp_status = response.status();
    let resp_headers = response.headers_mut();

    // simple implementation first:
    // This is for content we need to invalidate in the CDN level.
    // We don't care about content that is filename-hashed and can be cached
    // forever, or content that is not cached at all.
    //
    // Generally Fastly can either purge single URLs, or a whole service.
    // When you want to purge the cache for a bigger subset, but not everything, you need to "tag"
    // your content with surrogate keys when delivering it to Fastly for caching.
    // https://www.fastly.com/documentation/guides/full-site-delivery/purging/working-with-surrogate-keys/
    //
    // At some point we should extend this system and make it explicit, so in all places you return
    // a cache policy you also return these surrogate keys, probably based on the krate, release
    // or other things. For now we stick to invalidating the whole crate on all changes.
    //
    // For the first version I found an easy "hack" that doesn't need the full refactor across
    // all our handlers;
    // If the URL contains a crate name, we create a surrogate key based on that.
    // Since we always call the crate name (and only the crate name) `{name}` in our routes,
    // we're safe here. I added some debug assertions to ensure my assumptions are right, and
    // any change to these in the routes would lead to test failures.
    let cache_headers = if let Some(ref name) = param.name {
        // we could theoretically only run this part when cache_invalidatable_responses and
        // cache_headers.needs_cdn_invalidation are true,
        // but let's always to this validation and add the surrogate-key to know if
        // our "hack" still works.
        //
        // I didn't think through the possible edge-cases yet, but I feel safer
        // always adding a surrogate key if we have one.
        debug_assert!(
            matched_route
                .map(|matched_route| {
                    let matched_route = matched_route.as_str();
                    matched_route.starts_with("/crate/{name}")
                        || matched_route.starts_with("/{name}")
                })
                .unwrap_or(true),
            "there shouldn't be a name on any other routes"
        );
        if let Ok(krate_name) = name.parse::<KrateName>() {
            let keys = SurrogateKeys::from_iter_until_full(vec![krate_name.into()]);

            resp_headers.typed_insert(keys);

            // only allow caching in the CDN when we have a surrogate key to invalidate it later.
            // This is just the default for all routes that include a crate name.
            // Then we build  build & add the surrugate yet.
            // It's totally possible that this policy here then states NO_CACHING,
            // or FOREVER_IN_CDN_AND_BROWSER, where we wouln't need the surrogate key.
            &cache_headers
        } else if cache_headers.needs_cdn_invalidation {
            // This theoretically shouldn't happen, all current crate names would be valid
            // for surrogate keys, and the `KrateName` validation matches the crates.io crate
            // publish validation.
            // But I'll leave this error log here just in case, until I migrated to using the
            // `KrateName` type in all entrypoints (web, builds).
            if resp_status.is_success() || resp_status.is_redirection() {
                error!(
                    name = param.name,
                    ?cache_headers,
                    "failed to create surrogate key for crate, falling back to NO_CACHING"
                );
            }
            &NO_CACHING
        } else {
            &cache_headers
        }
    } else {
        debug_assert!(
            matched_route
                .map(|matched_route| {
                    let matched_route = matched_route.as_str();
                    !(matched_route.starts_with("/crate/{name}")
                        || matched_route.starts_with("/{name}"))
                })
                .unwrap_or(true),
            "for rustdoc & crate-detail routes the `name` param should always be present"
        );
        debug_assert!(
            !(config.cache_invalidatable_responses && cache_headers.needs_cdn_invalidation),
            "We got to a route without crate name, and a cache policy that needs invalidation.
             This doesn't work because Fastly only supports surrogate keys for partial
             invalidation."
        );

        // standard case, just use the cache policy, no surrogate keys needed.
        &cache_headers
    };

    cache_headers.set_on_response(resp_headers);

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::{
        AxumResponseTestExt as _, TestEnvironment, assert_cache_headers_eq,
        headers::{test_typed_decode, test_typed_encode},
    };
    use anyhow::{Context as _, Result};
    use axum::{Router, body::Body, http::Request, routing::get};
    use axum_extra::headers::CacheControl;
    use strum::IntoEnumIterator as _;
    use test_case::{test_case, test_matrix};
    use tower::{ServiceBuilder, ServiceExt as _};

    fn validate_cache_control(value: &HeaderValue) -> Result<()> {
        assert!(!value.as_bytes().is_empty());

        // first parse attempt.
        // The `CacheControl` typed header impl will just skip over unknown directives.
        let parsed: CacheControl = test_typed_decode(value.clone())?.unwrap();

        // So we just re-render it, re-parse and compare both.
        let re_rendered = test_typed_encode(parsed.clone());
        let re_parsed: CacheControl = test_typed_decode(re_rendered)?.unwrap();

        assert_eq!(parsed, re_parsed);

        Ok(())
    }

    #[test]
    fn test_const_response_consistency() {
        assert_eq!(
            FOREVER_IN_FASTLY_CDN.cache_control,
            NO_CACHING.cache_control
        );
    }

    #[test_matrix(
        [true, false],
        [Some(86400), None]
    )]
    fn test_validate_header_syntax_for_all_possible_combinations(
        cache_invalidatable_responses: bool,
        stale_while_revalidate: Option<u32>,
    ) -> Result<()> {
        let config = TestEnvironment::base_config()
            .cache_invalidatable_responses(cache_invalidatable_responses)
            .cache_control_stale_while_revalidate(stale_while_revalidate)
            .build()?;

        for policy in CachePolicy::iter() {
            let headers = policy.render(&config);

            if let Some(cache_control) = headers.cache_control {
                validate_cache_control(&cache_control).with_context(|| {
                    format!(
                        "couldn't validate Cache-Control header syntax for policy {:?}",
                        policy
                    )
                })?;
            }

            if let Some(surrogate_control) = headers.surrogate_control {
                validate_cache_control(&surrogate_control).with_context(|| {
                    format!(
                        "couldn't validate Surrogate-Control header syntax for policy {:?}",
                        policy
                    )
                })?;
            }
        }
        Ok(())
    }

    #[test_case(CachePolicy::NoCaching, Some("max-age=0"), None)]
    #[test_case(
        CachePolicy::NoStoreMustRevalidate,
        Some("no-cache, no-store, must-revalidate, max-age=0"),
        None
    )]
    #[test_case(
        CachePolicy::ForeverInCdnAndBrowser,
        Some("public, max-age=31104000, immutable"),
        None
    )]
    #[test_case(CachePolicy::ForeverInCdn, Some("max-age=0"), Some("max-age=31536000"))]
    #[test_case(
        CachePolicy::ForeverInCdnAndStaleInBrowser,
        Some("stale-while-revalidate=86400"),
        Some("max-age=31536000")
    )]
    fn render_fastly(
        cache: CachePolicy,
        cache_control: Option<&str>,
        surrogate_control: Option<&str>,
    ) -> Result<()> {
        let config = TestEnvironment::base_config().build()?;
        let headers = cache.render(&config);

        assert_eq!(
            headers.cache_control,
            cache_control.map(|s| HeaderValue::from_str(s).unwrap())
        );

        assert_eq!(
            headers.surrogate_control,
            surrogate_control.map(|s| HeaderValue::from_str(s).unwrap())
        );

        Ok(())
    }

    #[test]
    fn render_stale_without_config_fastly() -> Result<()> {
        let config = TestEnvironment::base_config()
            .cache_control_stale_while_revalidate(None)
            .build()?;

        let headers = CachePolicy::ForeverInCdnAndStaleInBrowser.render(&config);
        assert_eq!(headers, FOREVER_IN_FASTLY_CDN);

        Ok(())
    }

    #[test]
    fn render_stale_with_config_fastly() -> Result<()> {
        let config = TestEnvironment::base_config()
            .cache_control_stale_while_revalidate(Some(666))
            .build()?;

        let headers = CachePolicy::ForeverInCdnAndStaleInBrowser.render(&config);
        assert_eq!(headers.cache_control.unwrap(), "stale-while-revalidate=666");
        assert_eq!(
            headers.surrogate_control,
            FOREVER_IN_FASTLY_CDN.surrogate_control
        );

        Ok(())
    }

    #[test]
    fn render_forever_in_cdn_disabled_fastly() -> Result<()> {
        let config = TestEnvironment::base_config()
            .cache_invalidatable_responses(false)
            .build()?;

        let headers = CachePolicy::ForeverInCdn.render(&config);
        assert_eq!(headers.cache_control.unwrap(), "max-age=0");
        assert!(headers.surrogate_control.is_none());

        Ok(())
    }

    #[test]
    fn render_forever_in_cdn_or_stale_disabled_fastly() -> Result<()> {
        let config = TestEnvironment::base_config()
            .cache_invalidatable_responses(false)
            .build()?;

        let headers = CachePolicy::ForeverInCdnAndStaleInBrowser.render(&config);
        assert_eq!(headers.cache_control.unwrap(), "max-age=0");
        assert!(headers.surrogate_control.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_middleware_reacts_to_fastly_header_in_crate_route() -> Result<()> {
        let config = TestEnvironment::base_config()
            .cache_invalidatable_responses(true)
            .build()?;

        let app = Router::new()
            .route(
                "/{name}",
                get(move || async move { (Extension(CachePolicy::ForeverInCdn), "Hello, World!") }),
            )
            .layer(
                ServiceBuilder::new()
                    .layer(Extension(Arc::new(config)))
                    .layer(axum::middleware::from_fn(cache_middleware)),
            );

        let builder = Request::builder().uri("/krate");

        let response = app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await?;

        assert!(
            response.status().is_success(),
            "{}",
            response.text().await.unwrap(),
        );
        assert_cache_headers_eq(&response, &FOREVER_IN_FASTLY_CDN);

        Ok(())
    }

    #[tokio::test]
    async fn test_middleware_reacts_to_fastly_header_in_other_route() -> Result<()> {
        let config = TestEnvironment::base_config().build()?;

        let app = Router::new()
            .route(
                "/",
                get(move || async move {
                    (
                        Extension(CachePolicy::ForeverInCdnAndBrowser),
                        "Hello, World!",
                    )
                }),
            )
            .layer(
                ServiceBuilder::new()
                    .layer(Extension(Arc::new(config)))
                    .layer(axum::middleware::from_fn(cache_middleware)),
            );

        let builder = Request::builder().uri("/");

        let response = app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await?;

        assert!(
            response.status().is_success(),
            "{}",
            response.text().await.unwrap(),
        );

        // this cache policy leads to the same result in both CDNs
        assert_cache_headers_eq(&response, &FOREVER_IN_CDN_AND_BROWSER);

        Ok(())
    }
}
