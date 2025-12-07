use anyhow::{Result, bail};
use docs_rs_env_vars::{env, maybe_env, require_env};
use std::{path::PathBuf, sync::Arc, time::Duration};
use url::Url;

#[derive(Debug)]
pub struct Config {
    pub prefix: PathBuf,

    pub database: Arc<docs_rs_database::Config>,
    pub watcher: Arc<docs_rs_watcher::Config>,
    pub build_queue: Arc<docs_rs_build_queue::Config>,
    pub storage: Arc<docs_rs_storage::Config>,

    // Access token for APIs for crates.io (careful: use
    // constant_time_eq for comparisons!)
    pub(crate) cratesio_token: Option<String>,

    // amount of retries for external API calls, mostly crates.io
    pub crates_io_api_call_retries: u32,

    // request timeout in seconds
    pub(crate) request_timeout: Option<Duration>,
    pub(crate) report_request_timeouts: bool,

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

    // Where to collect metrics for the metrics initiative.
    // When empty, we won't collect metrics.
    pub(crate) compiler_metrics_collection_path: Option<PathBuf>,

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

    pub(crate) build_workspace_reinitialization_interval: Duration,

    // Build params
    pub(crate) build_attempts: u16,
    pub(crate) delay_between_build_attempts: Duration,
    pub(crate) rustwide_workspace: PathBuf,
    pub(crate) temp_dir: PathBuf,
    pub(crate) inside_docker: bool,
    pub(crate) docker_image: Option<String>,
    pub(crate) build_cpu_limit: Option<u32>,
    pub(crate) build_default_memory_limit: Option<usize>,
    pub(crate) include_default_targets: bool,
    pub(crate) disable_memory_limit: bool,

    pub(crate) opentelemetry: docs_rs_opentelemetry::Config,
}

impl Config {
    pub fn from_env() -> Result<Config> {
        let prefix: PathBuf = require_env("DOCSRS_PREFIX")?;
        let temp_dir = prefix.join("tmp");

        Ok(Self {
            prefix,
            database: docs_rs_database::Config::from_environment()?.into(),
            watcher: docs_rs_watcher::Config::from_environment()?.into(),
            build_queue: docs_rs_build_queue::Config::from_environment()?.into(),
            storage: docs_rs_storage::Config::from_environment()?.into(),
            opentelemetry: docs_rs_opentelemetry::Config::from_environment()?.into(),

            build_attempts: env("DOCSRS_BUILD_ATTEMPTS", 5u16)?,
            delay_between_build_attempts: Duration::from_secs(env::<u64>(
                "DOCSRS_DELAY_BETWEEN_BUILD_ATTEMPTS",
                60,
            )?),
            crates_io_api_call_retries: env("DOCSRS_CRATESIO_API_CALL_RETRIES", 3u32)?,
            cratesio_token: maybe_env("DOCSRS_CRATESIO_TOKEN")?,
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
            compiler_metrics_collection_path: maybe_env("DOCSRS_COMPILER_METRICS_PATH")?,
            temp_dir: temp_dir,
            rustwide_workspace: env("DOCSRS_RUSTWIDE_WORKSPACE", PathBuf::from(".workspace"))?,
            inside_docker: env("DOCSRS_DOCKER", false)?,
            docker_image: maybe_env("DOCSRS_LOCAL_DOCKER_IMAGE")?
                .or(maybe_env("DOCSRS_DOCKER_IMAGE")?),
            build_cpu_limit: maybe_env("DOCSRS_BUILD_CPU_LIMIT")?,
            build_default_memory_limit: maybe_env("DOCSRS_BUILD_DEFAULT_MEMORY_LIMIT")?,
            include_default_targets: env("DOCSRS_INCLUDE_DEFAULT_TARGETS", true)?,
            disable_memory_limit: env("DOCSRS_DISABLE_MEMORY_LIMIT", false)?,
            build_workspace_reinitialization_interval: Duration::from_secs(env(
                "DOCSRS_BUILD_WORKSPACE_REINITIALIZATION_INTERVAL",
                86400,
            )?),
        })
    }
}
