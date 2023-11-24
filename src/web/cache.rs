use crate::config::Config;
use axum::{
    http::Request as AxumHttpRequest, middleware::Next, response::Response as AxumResponse,
};
use http::{header::CACHE_CONTROL, HeaderValue};
use std::sync::Arc;

pub static NO_CACHING: HeaderValue = HeaderValue::from_static("max-age=0");
pub static SHORT: HeaderValue = HeaderValue::from_static("max-age=60");

pub static NO_STORE_MUST_REVALIDATE: HeaderValue =
    HeaderValue::from_static("no-cache, no-store, must-revalidate, max-age=0");

pub static FOREVER_IN_CDN_AND_BROWSER: HeaderValue = HeaderValue::from_static("max-age=31104000");

/// defines the wanted caching behaviour for a web response.
#[derive(Debug)]
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
    /// while protecting agains spikes in traffic.
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
    /// cache forver in the CDN, but allow stale content in the browser.
    /// Example: rustdoc pages with the version in their URL.
    /// A browser will show the stale content while getting the up-to-date
    /// version from the origin server in the background.
    /// This helps building a PWA.
    ForeverInCdnAndStaleInBrowser,
}

impl CachePolicy {
    pub fn render(&self, config: &Config) -> Option<HeaderValue> {
        match *self {
            CachePolicy::NoCaching => Some(NO_CACHING.clone()),
            CachePolicy::NoStoreMustRevalidate => Some(NO_STORE_MUST_REVALIDATE.clone()),
            CachePolicy::ShortInCdnAndBrowser => Some(SHORT.clone()),
            CachePolicy::ForeverInCdnAndBrowser => Some(FOREVER_IN_CDN_AND_BROWSER.clone()),
            CachePolicy::ForeverInCdn => {
                if config.cache_invalidatable_responses {
                    // A missing `max-age` or `s-maxage` in the Cache-Control header will lead to
                    // CloudFront using the default TTL, while the browser not seeing any caching header.
                    // This means we can have the CDN caching the documentation while just
                    // issuing a purge after a build.
                    // https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/Expiration.html#ExpirationDownloadDist
                    None
                } else {
                    Some(NO_CACHING.clone())
                }
            }
            CachePolicy::ForeverInCdnAndStaleInBrowser => {
                if config.cache_invalidatable_responses {
                    config
                        .cache_control_stale_while_revalidate
                        .map(|seconds| format!("stale-while-revalidate={seconds}").parse().unwrap())
                } else {
                    Some(NO_CACHING.clone())
                }
            }
        }
    }
}

pub(crate) async fn cache_middleware<B>(req: AxumHttpRequest<B>, next: Next<B>) -> AxumResponse {
    let config = req
        .extensions()
        .get::<Arc<Config>>()
        .cloned()
        .expect("missing config extension in request");

    let mut response = next.run(req).await;

    let cache = response
        .extensions()
        .get::<CachePolicy>()
        .unwrap_or(&CachePolicy::NoCaching);

    if cfg!(test) {
        assert!(
            !response.headers().contains_key(CACHE_CONTROL),
            "handlers should never set their own caching headers and only use CachePolicy to control caching."
        );
    }

    if let Some(cache_directive) = cache.render(&config) {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, cache_directive);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::wrapper;
    use test_case::test_case;

    #[test_case(CachePolicy::NoCaching, Some("max-age=0"))]
    #[test_case(
        CachePolicy::NoStoreMustRevalidate,
        Some("no-cache, no-store, must-revalidate, max-age=0")
    )]
    #[test_case(CachePolicy::ForeverInCdnAndBrowser, Some("max-age=31104000"))]
    #[test_case(CachePolicy::ForeverInCdn, None)]
    #[test_case(
        CachePolicy::ForeverInCdnAndStaleInBrowser,
        Some("stale-while-revalidate=86400")
    )]
    fn render(cache: CachePolicy, expected: Option<&str>) {
        wrapper(|env| {
            assert_eq!(
                cache.render(&env.config()),
                expected.map(|s| HeaderValue::from_str(s).unwrap())
            );
            Ok(())
        });
    }

    #[test]
    fn render_stale_without_config() {
        wrapper(|env| {
            env.override_config(|config| config.cache_control_stale_while_revalidate = None);

            assert!(CachePolicy::ForeverInCdnAndStaleInBrowser
                .render(&env.config())
                .is_none());
            Ok(())
        });
    }

    #[test]
    fn render_stale_with_config() {
        wrapper(|env| {
            env.override_config(|config| {
                config.cache_control_stale_while_revalidate = Some(666);
            });

            assert_eq!(
                CachePolicy::ForeverInCdnAndStaleInBrowser
                    .render(&env.config())
                    .unwrap(),
                "stale-while-revalidate=666"
            );
            Ok(())
        });
    }

    #[test]
    fn render_forever_in_cdn_disabled() {
        wrapper(|env| {
            env.override_config(|config| {
                config.cache_invalidatable_responses = false;
            });

            assert_eq!(
                CachePolicy::ForeverInCdn.render(&env.config()).unwrap(),
                "max-age=0"
            );
            Ok(())
        });
    }

    #[test]
    fn render_forever_in_cdn_or_stale_disabled() {
        wrapper(|env| {
            env.override_config(|config| {
                config.cache_invalidatable_responses = false;
            });

            assert_eq!(
                CachePolicy::ForeverInCdnAndStaleInBrowser
                    .render(&env.config())
                    .unwrap(),
                "max-age=0"
            );
            Ok(())
        });
    }
}
