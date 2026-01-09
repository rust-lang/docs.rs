use anyhow::Result;
use docs_rs_config::AppConfig;
use docs_rs_env_vars::maybe_env;
use std::time::Duration;

#[derive(Debug, bon::Builder)]
#[builder(on(_, overwritable))]
pub struct Config {
    // Access token for APIs for crates.io
    // (careful: use constant_time_eq for comparisons!)
    pub(crate) cratesio_token: Option<String>,

    // request timeout in seconds
    #[builder(with = |secs: u64| Duration::from_secs(secs))]
    pub(crate) request_timeout: Option<Duration>,
    #[builder(default)]
    pub(crate) report_request_timeouts: bool,

    // The most memory that can be used to parse an HTML file
    // LOL HTML only uses as much memory as the size of the start tag!
    // https://github.com/rust-lang/docs.rs/pull/930#issuecomment-667729380
    #[builder(default = 5 * 1024 * 1024usize)]
    pub(crate) max_parse_memory: usize,

    /// amount of threads for CPU intensive rendering
    #[builder(default = num_cpus::get())]
    pub(crate) render_threads: usize,

    // random crate search generates a number of random IDs to
    // efficiently find a random crate with > 100 GH stars.
    // The amount depends on the ratio of crates with >100 stars
    // to the count of all crates.
    // At the time of creating this setting, it is set to
    // `500` for a ratio of 7249 over 54k crates.
    // For unit-tests the number has to be higher.
    #[builder(default = 500u32)]
    pub(crate) random_crate_search_view_size: u32,

    // Content Security Policy
    #[builder(default)]
    pub(crate) csp_report_only: bool,

    // Cache-Control header, for versioned URLs.
    // If both are absent, don't generate the header. If only one is present,
    // generate just that directive. Values are in seconds.
    pub(crate) cache_control_stale_while_revalidate: Option<u32>,

    // Activate full page caching.
    // When disabled, we still cache static assets.
    // This only affects pages that depend on invalidations to work.
    #[builder(default = true)]
    pub(crate) cache_invalidatable_responses: bool,
}

use config_builder::State;

impl<S: State> ConfigBuilder<S> {
    pub(crate) fn load_environment(self) -> Result<ConfigBuilder<S>> {
        Ok(self
            .maybe_cratesio_token(maybe_env("DOCSRS_CRATESIO_TOKEN")?)
            .maybe_max_parse_memory(maybe_env("DOCSRS_MAX_PARSE_MEMORY")?)
            .maybe_render_threads(maybe_env("DOCSRS_RENDER_THREADS")?)
            .maybe_request_timeout(maybe_env("DOCSRS_REQUEST_TIMEOUT")?)
            .maybe_report_request_timeouts(maybe_env("DOCSRS_REPORT_REQUEST_TIMEOUTS")?)
            .maybe_random_crate_search_view_size(maybe_env("DOCSRS_RANDOM_CRATE_SEARCH_VIEW_SIZE")?)
            .maybe_csp_report_only(maybe_env("DOCSRS_CSP_REPORT_ONLY")?)
            .maybe_cache_control_stale_while_revalidate(maybe_env(
                "CACHE_CONTROL_STALE_WHILE_REVALIDATE",
            )?)
            .maybe_cache_invalidatable_responses(maybe_env(
                "DOCSRS_CACHE_INVALIDATEABLE_RESPONSES",
            )?))
    }

    #[cfg(test)]
    #[allow(clippy::type_complexity)]
    pub(crate) fn test_config(self) -> Result<ConfigBuilder<S>> {
        Ok(self
            .load_environment()?
            // set stale content serving so Cache::ForeverInCdn and Cache::ForeverInCdnAndStaleInBrowser
            // are actually different.
            .cache_control_stale_while_revalidate(86400))
    }
}

impl AppConfig for Config {
    fn from_environment() -> Result<Self> {
        Ok(Self::builder().load_environment()?.build())
    }

    #[cfg(test)]
    fn test_config() -> Result<Self> {
        Ok(Self::builder().test_config()?.build())
    }
}
