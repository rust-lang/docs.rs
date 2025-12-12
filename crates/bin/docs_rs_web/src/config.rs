use docs_rs_env_vars::{env, maybe_env, require_env};
use std::{path::PathBuf, time::Duration};
use url::Url;

#[derive(Debug)]
pub struct Config {
    // Access token for APIs for crates.io (careful: use
    // constant_time_eq for comparisons!)
    pub(crate) cratesio_token: Option<String>,
    // request timeout in seconds
    pub(crate) request_timeout: Option<Duration>,
    pub(crate) report_request_timeouts: bool,
    //
    // Max size of the files served by the docs.rs frontend
    pub(crate) max_file_size: usize,
    pub(crate) max_file_size_html: usize,
    // The most memory that can be used to parse an HTML file
    pub(crate) max_parse_memory: usize,
    /// amount of threads for CPU intensive rendering
    pub(crate) render_threads: usize,
    // random crate search generates a number of random IDs to
    // efficiently find a random crate with > 100 GH stars.
    // The amount depends on the ratio of crates with >100 stars
    // to the count of all crates.
    // At the time of creating this setting, it is set to
    // `500` for a ratio of 7249 over 54k crates.
    // For unit-tests the number has to be higher.
    pub(crate) random_crate_search_view_size: u32,

    // Content Security Policy
    pub(crate) csp_report_only: bool,

    // Cache-Control header, for versioned URLs.
    // If both are absent, don't generate the header. If only one is present,
    // generate just that directive. Values are in seconds.
    pub(crate) cache_control_stale_while_revalidate: Option<u32>,

    // Activate full page caching.
    // When disabled, we still cache static assets.
    // This only affects pages that depend on invalidations to work.
    pub(crate) cache_invalidatable_responses: bool,

    pub(crate) storage: docs_rs_storage::Config,
    pub(crate) build_utils_config: docs_rs_build_utils::Config,
}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {
        let prefix: PathBuf = require_env("DOCSRS_PREFIX")?;
        Ok(Self {
            cratesio_token: maybe_env("DOCSRS_CRATESIO_TOKEN")?,
            max_file_size: env("DOCSRS_MAX_FILE_SIZE", 50 * 1024 * 1024)?,
            max_file_size_html: env("DOCSRS_MAX_FILE_SIZE_HTML", 50 * 1024 * 1024)?,
            // LOL HTML only uses as much memory as the size of the start tag!
            // https://github.com/rust-lang/docs.rs/pull/930#issuecomment-667729380
            max_parse_memory: env("DOCSRS_MAX_PARSE_MEMORY", 5 * 1024 * 1024)?,
            render_threads: env("DOCSRS_RENDER_THREADS", num_cpus::get())?,
            request_timeout: maybe_env::<u64>("DOCSRS_REQUEST_TIMEOUT")?.map(Duration::from_secs),
            report_request_timeouts: env("DOCSRS_REPORT_REQUEST_TIMEOUTS", false)?,
            random_crate_search_view_size: env("DOCSRS_RANDOM_CRATE_SEARCH_VIEW_SIZE", 500)?,
            csp_report_only: env("DOCSRS_CSP_REPORT_ONLY", false)?,
            cache_control_stale_while_revalidate: maybe_env(
                "CACHE_CONTROL_STALE_WHILE_REVALIDATE",
            )?,
            cache_invalidatable_responses: env("DOCSRS_CACHE_INVALIDATEABLE_RESPONSES", true)?,
            storage: docs_rs_storage::Config::from_environment()?,
            build_utils_config: docs_rs_build_utils::Config::from_environment()?,
        })
    }
}
