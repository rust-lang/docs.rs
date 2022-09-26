use super::STATIC_FILE_CACHE_DURATION;
use crate::config::Config;
use iron::{
    headers::{CacheControl, CacheDirective},
    AfterMiddleware, IronResult, Request, Response,
};

#[cfg(test)]
pub const NO_CACHE: &str = "max-age=0";

/// defines the wanted caching behaviour for a web response.
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
    /// cache forever in browser & CDN.  
    /// Valid when you have hashed / versioned filenames and every rebuild would
    /// change the filename.
    ForeverInCdnAndBrowser,
    /// cache forever in CDN, but not in the browser.
    /// Since we control the CDN we can actively purge content that is cached like
    /// this, for example after building a crate.
    /// Example usage: `/latest/` rustdoc pages and their redirects.
    ForeverOnlyInCdn,
    /// cache forver in the CDN, but allow stale content in the browser.
    /// Example: rustdoc pages with the version in their URL.
    /// A browser will show the stale content while getting the up-to-date
    /// version from the origin server in the background.
    /// This helps building a PWA.
    ForeverInCdnAndStaleInBrowser,
}

impl CachePolicy {
    pub fn render(&self, config: &Config) -> Vec<CacheDirective> {
        match *self {
            CachePolicy::NoCaching => {
                vec![CacheDirective::MaxAge(0)]
            }
            CachePolicy::NoStoreMustRevalidate => {
                vec![
                    CacheDirective::NoCache,
                    CacheDirective::NoStore,
                    CacheDirective::MustRevalidate,
                    CacheDirective::MaxAge(0),
                ]
            }
            CachePolicy::ForeverInCdnAndBrowser => {
                vec![
                    CacheDirective::Public,
                    CacheDirective::MaxAge(STATIC_FILE_CACHE_DURATION as u32),
                ]
            }
            CachePolicy::ForeverOnlyInCdn => {
                // A missing `max-age` or `s-maxage` in the Cache-Control header will lead to
                // CloudFront using the default TTL, while the browser not seeing any caching header.
                // This means we can have the CDN caching the documentation while just
                // issuing a purge after a build.
                // https://docs.aws.amazon.com/AmazonCloudFront/latest/DeveloperGuide/Expiration.html#ExpirationDownloadDist
                vec![CacheDirective::Public]
            }
            CachePolicy::ForeverInCdnAndStaleInBrowser => {
                let mut directives = CachePolicy::ForeverOnlyInCdn.render(config);
                if let Some(seconds) = config.cache_control_stale_while_revalidate {
                    directives.push(CacheDirective::Extension(
                        "stale-while-revalidate".to_string(),
                        Some(seconds.to_string()),
                    ));
                }
                directives
            }
        }
    }
}

impl iron::typemap::Key for CachePolicy {
    type Value = CachePolicy;
}

/// Middleware to ensure a correct cache-control header.
/// The default is an explicit "never cache" header, which
/// can be adapted via:
/// ```ignore
///  resp.extensions.insert::<CachePolicy>(CachePolicy::ForeverOnlyInCdn);
///  # change Cache::ForeverOnlyInCdn into the cache polity you want to have
/// ```
/// in a handler function.
pub(super) struct CacheMiddleware;

impl AfterMiddleware for CacheMiddleware {
    fn after(&self, req: &mut Request, mut res: Response) -> IronResult<Response> {
        let config = req.extensions.get::<Config>().expect("missing config");
        let cache = res
            .extensions
            .get::<CachePolicy>()
            .unwrap_or(&CachePolicy::NoCaching);

        if cfg!(test) {
            // handlers should never set their own caching headers and
            // only use the caching header templates above.
            assert!(!res.headers.has::<CacheControl>());
        }

        res.headers.set(CacheControl(cache.render(config)));
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::wrapper;
    use iron::headers::CacheControl;
    use test_case::test_case;

    #[test_case(CachePolicy::NoCaching, "max-age=0")]
    #[test_case(
        CachePolicy::NoStoreMustRevalidate,
        "no-cache, no-store, must-revalidate, max-age=0"
    )]
    #[test_case(CachePolicy::ForeverInCdnAndBrowser, "public, max-age=31104000")]
    #[test_case(CachePolicy::ForeverOnlyInCdn, "public")]
    #[test_case(
        CachePolicy::ForeverInCdnAndStaleInBrowser,
        "public, stale-while-revalidate=86400"
    )]
    fn render(cache: CachePolicy, expected: &str) {
        wrapper(|env| {
            assert_eq!(
                CacheControl(cache.render(&env.config())).to_string(),
                expected
            );
            Ok(())
        });
    }

    #[test]
    fn render_stale_without_config() {
        wrapper(|env| {
            env.override_config(|config| config.cache_control_stale_while_revalidate = None);

            assert_eq!(
                CacheControl(CachePolicy::ForeverInCdnAndStaleInBrowser.render(&env.config()))
                    .to_string(),
                "public"
            );
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
                CacheControl(CachePolicy::ForeverInCdnAndStaleInBrowser.render(&env.config()))
                    .to_string(),
                "public, stale-while-revalidate=666"
            );
            Ok(())
        });
    }
}
