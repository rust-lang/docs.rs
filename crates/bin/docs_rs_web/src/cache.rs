use crate::config::Config;
use axum::{
    Extension, extract::Request as AxumHttpRequest, middleware::Next,
    response::Response as AxumResponse,
};
use axum_extra::headers::HeaderMapExt as _;
use docs_rs_headers::{SURROGATE_CONTROL, SURROGATE_KEY, SurrogateKey, SurrogateKeys};
use http::{
    HeaderMap, HeaderValue, StatusCode,
    header::{CACHE_CONTROL, ETAG},
};
use std::sync::Arc;
use tracing::error;

/// a surrogate key that is attached to _all_ content.
/// This enables us to use the fastly "soft purge" for everything.
pub const SURROGATE_KEY_ALL: SurrogateKey = SurrogateKey::from_static("all");

/// A surrogate key that we apply to content that is static and should be
/// invalidated everything we deploy a new version of docs.rs.
pub const SURROGATE_KEY_DOCSRS_STATIC: SurrogateKey = SurrogateKey::from_static("docs-rs-static");

/// cache poicy for static assets like rustdoc files or build assets.
pub const STATIC_ASSET_CACHE_POLICY: CachePolicy = CachePolicy::ForeverInCdnAndBrowser;

#[derive(Debug, Clone, PartialEq)]
pub struct ResponseCacheHeaders {
    pub cache_control: Option<HeaderValue>,
    pub surrogate_control: Option<HeaderValue>,
    pub surrogate_keys: Option<SurrogateKeys>,
    pub needs_cdn_invalidation: bool,

    // whether to add a global surrogate key to this response.
    // Needed only when we actually cache something.
    pub is_caching_something: bool,
}

impl ResponseCacheHeaders {
    fn set_on_response(self, headers: &mut HeaderMap) {
        if let Some(cache_control) = self.cache_control {
            headers.insert(CACHE_CONTROL, cache_control);
        }
        if let Some(surrogate_control) = self.surrogate_control {
            headers.insert(&SURROGATE_CONTROL, surrogate_control);
        }
        if let Some(surrogate_keys) = self.surrogate_keys {
            headers.typed_insert(surrogate_keys);
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
    surrogate_keys: None,
    needs_cdn_invalidation: false,
    is_caching_something: false,
};

/// Cache for a short time in the browser & in the CDN.
/// Helps protecting against traffic spikes.
static SHORT: ResponseCacheHeaders = ResponseCacheHeaders {
    cache_control: Some(HeaderValue::from_static("public, max-age=60")),
    surrogate_control: None,
    surrogate_keys: None,
    needs_cdn_invalidation: false,
    is_caching_something: true,
};

/// don't cache, don't even store. Never. Ever.
static NO_STORE_MUST_REVALIDATE: ResponseCacheHeaders = ResponseCacheHeaders {
    cache_control: Some(HeaderValue::from_static(
        "no-cache, no-store, must-revalidate, max-age=0",
    )),
    surrogate_control: None,
    surrogate_keys: None,
    needs_cdn_invalidation: false,
    is_caching_something: false,
};

static FOREVER_IN_FASTLY_CDN: ResponseCacheHeaders = ResponseCacheHeaders {
    // explicitly forbid browser caching, same as NO_CACHING above.
    cache_control: Some(HeaderValue::from_static("max-age=0")),

    // set `surrogate-control`, cache forever in the CDN
    // https://www.fastly.com/documentation/reference/http/http-headers/Surrogate-Control/
    //
    // TODO: evaluate if we can / should set `stale-while-revalidate` or `stale-if-error` here,
    // especially in combination with our fastly compute service.
    // https://www.fastly.com/documentation/guides/concepts/edge-state/cache/stale/
    surrogate_control: Some(HeaderValue::from_static("max-age=31536000")),
    surrogate_keys: None,

    needs_cdn_invalidation: true,
    is_caching_something: true,
};

/// cache forever in browser & CDN.
/// Only usable for content with unique filenames.
///
/// We use this policy mostly for static files, rustdoc toolchain assets,
/// or build assets.
static FOREVER_IN_CDN_AND_BROWSER: ResponseCacheHeaders = ResponseCacheHeaders {
    cache_control: Some(HeaderValue::from_static(
        "public, max-age=31104000, immutable",
    )),
    surrogate_control: None,
    surrogate_keys: None,
    needs_cdn_invalidation: false,
    is_caching_something: true,
};

/// defines the wanted caching behaviour for a web response.
#[derive(Debug, Clone)]
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
    /// Note: The CDN (Fastly) needs a list of surrogate keys ( = tags )to be able to purge a
    /// subset of the pages
    /// Example usage: `/latest/` rustdoc pages and their redirects.
    ForeverInCdn(SurrogateKeys),
    /// cache forever in the CDN, but allow stale content in the browser.
    /// Note: The CDN (Fastly) needs a list of surrogate keys ( = tags )to be able to purge a
    /// subset of the pages
    /// Example: rustdoc pages with the version in their URL.
    /// A browser will show the stale content while getting the up-to-date
    /// version from the origin server in the background.
    /// This helps building a PWA.
    ForeverInCdnAndStaleInBrowser(SurrogateKeys),
}

impl CachePolicy {
    pub fn render(self, config: &Config) -> anyhow::Result<ResponseCacheHeaders> {
        let mut headers = match self {
            CachePolicy::NoCaching => NO_CACHING.clone(),
            CachePolicy::NoStoreMustRevalidate => NO_STORE_MUST_REVALIDATE.clone(),
            CachePolicy::ShortInCdnAndBrowser => SHORT.clone(),
            CachePolicy::ForeverInCdnAndBrowser => FOREVER_IN_CDN_AND_BROWSER.clone(),
            CachePolicy::ForeverInCdn(surrogate_keys) => {
                if config.cache_invalidatable_responses {
                    let mut cache_headers = FOREVER_IN_FASTLY_CDN.clone();

                    cache_headers
                        .surrogate_keys
                        .get_or_insert_with(SurrogateKeys::new)
                        .try_extend(surrogate_keys.into_iter())?;

                    cache_headers
                } else {
                    NO_CACHING.clone()
                }
            }
            CachePolicy::ForeverInCdnAndStaleInBrowser(surrogate_keys) => {
                // when caching invalidatable responses is disabled, this results in NO_CACHING
                let mut forever_in_cdn =
                    CachePolicy::ForeverInCdn(surrogate_keys).render(config)?;

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
        };

        if headers.is_caching_something {
            headers
                .surrogate_keys
                .get_or_insert_with(SurrogateKeys::new)
                .try_extend([SURROGATE_KEY_ALL])?;
        }

        Ok(headers)
    }
}

pub(crate) async fn cache_middleware(
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
        .extensions_mut()
        .remove::<CachePolicy>()
        .unwrap_or(CachePolicy::NoCaching);

    let cache_headers = match cache_policy.render(&config) {
        Ok(headers) => headers,
        Err(e) => {
            error!(?e, "couldn't render cache headers for policy");
            NO_CACHING.clone()
        }
    };

    cache_headers.set_on_response(response.headers_mut());
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        AxumResponseTestExt as _, assert_cache_headers_eq,
        headers::{test_typed_decode, test_typed_encode},
    };
    use anyhow::{Context as _, Result};
    use axum::{Router, body::Body, routing::get};
    use axum_extra::headers::CacheControl;
    use docs_rs_config::AppConfig as _;
    use http::Request;
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
        let config = Config::builder()
            .test_config()?
            .cache_invalidatable_responses(cache_invalidatable_responses)
            .maybe_cache_control_stale_while_revalidate(stale_while_revalidate)
            .build();

        fn validate_headers(headers: &ResponseCacheHeaders) -> Result<()> {
            if let Some(ref cache_control) = headers.cache_control {
                validate_cache_control(cache_control)
                    .context("couldn't validate Cache-Control header syntax")?;
            }

            if let Some(ref surrogate_control) = headers.surrogate_control {
                validate_cache_control(surrogate_control)
                    .context("couldn't validate Surrogate-Control header syntax")?;
            }
            Ok(())
        }

        use CachePolicy::*;

        let key = SurrogateKey::from_static("something");

        for policy in [
            NoCaching,
            NoStoreMustRevalidate,
            ShortInCdnAndBrowser,
            ForeverInCdnAndBrowser,
            ForeverInCdn(key.clone().into()),
            ForeverInCdnAndStaleInBrowser(key.clone().into()),
        ] {
            let headers = policy.render(&config)?;
            validate_headers(&headers)?;
        }

        Ok(())
    }

    #[test_case(CachePolicy::NoCaching, Some("max-age=0"), None, false)]
    #[test_case(
        CachePolicy::NoStoreMustRevalidate,
        Some("no-cache, no-store, must-revalidate, max-age=0"),
        None,
        false
    )]
    #[test_case(
        CachePolicy::ForeverInCdnAndBrowser,
        Some("public, max-age=31104000, immutable"),
        None,
        true
    )]
    fn render_fastly(
        cache: CachePolicy,
        cache_control: Option<&str>,
        surrogate_control: Option<&str>,
        needs_global_surrogate_key: bool,
    ) -> Result<()> {
        let config = Config::test_config()?;
        let headers = cache.render(&config)?;

        assert_eq!(
            headers.cache_control,
            cache_control.map(|s| HeaderValue::from_str(s).unwrap())
        );

        assert_eq!(
            headers.surrogate_control,
            surrogate_control.map(|s| HeaderValue::from_str(s).unwrap())
        );

        if needs_global_surrogate_key {
            assert_eq!(headers.surrogate_keys.unwrap(), SURROGATE_KEY_ALL.into());
        } else {
            assert!(headers.surrogate_keys.is_none());
        }

        Ok(())
    }

    #[test]
    fn render_fastly_forever_in_cdn() -> Result<()> {
        let config = Config::test_config()?;
        // this surrogate key is user-defined, identifies the crate.
        let key = SurrogateKey::from_static("something");
        let headers = CachePolicy::ForeverInCdn(key.clone().into()).render(&config)?;

        // browser or other proxies: mostly no caching
        assert_eq!(
            headers.cache_control,
            Some(HeaderValue::from_static("max-age=0"))
        );

        // CDN: cache forever.
        // Fastly will completely ignore cache-control if it finds surrogate-control.
        assert_eq!(
            headers.surrogate_control,
            Some(HeaderValue::from_static("max-age=31536000"))
        );

        // both: our key + the global "all" key.
        // So we can purge the CDN for these keys.
        assert_eq!(
            headers.surrogate_keys.unwrap(),
            SurrogateKeys::try_from_iter([key, SURROGATE_KEY_ALL]).unwrap()
        );

        Ok(())
    }

    #[test]
    fn render_fastly_forever_in_cdn_stale_in_browser() -> Result<()> {
        let config = Config::test_config()?;
        let key = SurrogateKey::from_static("something");
        let headers =
            CachePolicy::ForeverInCdnAndStaleInBrowser(key.clone().into()).render(&config)?;

        assert_eq!(
            headers.cache_control,
            Some(HeaderValue::from_static("stale-while-revalidate=86400"))
        );
        assert_eq!(
            headers.surrogate_control,
            Some(HeaderValue::from_static("max-age=31536000"))
        );

        assert_eq!(
            headers.surrogate_keys.unwrap(),
            SurrogateKeys::try_from_iter([key, SURROGATE_KEY_ALL]).unwrap()
        );

        Ok(())
    }

    #[test]
    fn render_stale_without_config_fastly() -> Result<()> {
        let config = Config::builder()
            .test_config()?
            .maybe_cache_control_stale_while_revalidate(None)
            .build();

        let key = SurrogateKey::from_static("something");
        let mut headers =
            CachePolicy::ForeverInCdnAndStaleInBrowser(key.clone().into()).render(&config)?;
        assert_eq!(
            headers.surrogate_keys.take().unwrap(),
            SurrogateKeys::try_from_iter([key, SURROGATE_KEY_ALL]).unwrap()
        );
        assert_eq!(headers, FOREVER_IN_FASTLY_CDN);

        Ok(())
    }

    #[test]
    fn render_stale_with_config_fastly() -> Result<()> {
        let config = Config::builder()
            .test_config()?
            .cache_control_stale_while_revalidate(666)
            .build();

        let key = SurrogateKey::from_static("something");
        let headers =
            CachePolicy::ForeverInCdnAndStaleInBrowser(key.clone().into()).render(&config)?;
        assert_eq!(headers.cache_control.unwrap(), "stale-while-revalidate=666");
        assert_eq!(
            headers.surrogate_control,
            FOREVER_IN_FASTLY_CDN.surrogate_control
        );
        assert_eq!(
            headers.surrogate_keys.unwrap(),
            SurrogateKeys::try_from_iter([key, SURROGATE_KEY_ALL]).unwrap()
        );

        Ok(())
    }

    #[test]
    fn render_forever_in_cdn_disabled_fastly() -> Result<()> {
        let config = Config::builder()
            .test_config()?
            .cache_invalidatable_responses(false)
            .build();

        let key = SurrogateKey::from_static("something");
        let headers = CachePolicy::ForeverInCdn(key.into()).render(&config)?;
        assert_eq!(headers.cache_control.unwrap(), "max-age=0");
        assert!(headers.surrogate_control.is_none());
        assert!(headers.surrogate_keys.is_none());

        Ok(())
    }

    #[test]
    fn render_forever_in_cdn_or_stale_disabled_fastly() -> Result<()> {
        let config = Config::builder()
            .test_config()?
            .cache_invalidatable_responses(false)
            .build();

        let key = SurrogateKey::from_static("something");
        let headers = CachePolicy::ForeverInCdnAndStaleInBrowser(key.into()).render(&config)?;
        assert_eq!(headers.cache_control.unwrap(), "max-age=0");
        assert!(headers.surrogate_control.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_middleware_reacts_to_fastly_header_in_crate_route() -> Result<()> {
        let config = Arc::new(
            Config::builder()
                .test_config()?
                .cache_invalidatable_responses(true)
                .build(),
        );

        let key = SurrogateKey::from_static("krate");
        let policy = CachePolicy::ForeverInCdn(key.clone().into());
        let app = Router::new()
            .route(
                "/{name}",
                get({
                    let policy = policy.clone();
                    move || async move { (Extension(policy), "Hello, World!") }
                }),
            )
            .layer(
                ServiceBuilder::new()
                    .layer(Extension(config.clone()))
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
        assert_cache_headers_eq(&response, &policy.render(&config)?);

        Ok(())
    }

    #[tokio::test]
    async fn test_middleware_reacts_to_fastly_header_in_other_route() -> Result<()> {
        let config = Arc::new(Config::test_config()?);

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
                    .layer(Extension(config.clone()))
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
        assert_cache_headers_eq(
            &response,
            &CachePolicy::ForeverInCdnAndBrowser.render(&config)?,
        );

        Ok(())
    }
}
