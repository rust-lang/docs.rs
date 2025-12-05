use anyhow::{Result, bail};
use docs_rs_env_vars::{env, maybe_env, require_env};
use std::{
    io,
    path::{self, PathBuf},
    time::Duration,
};
use url::Url;

#[derive(Debug, derive_builder::Builder)]
#[builder(pattern = "owned")]
pub struct Config {
    pub prefix: PathBuf,

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

    /// Fastly API host, typically only overwritten for testing
    pub fastly_api_host: Url,

    /// Fastly API token for purging the services below.
    pub fastly_api_token: Option<String>,

    /// fastly service SID for the main domain
    pub fastly_service_sid: Option<String>,

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

    // automatic rebuild configuration
    pub(crate) max_queued_rebuilds: Option<u16>,

    // opentelemetry endpoint to send OTLP to
    pub(crate) opentelemetry_endpoint: Option<Url>,
}

impl Config {
    pub fn from_env() -> Result<ConfigBuilder> {
        let old_vars = [
            ("CRATESFYI_PREFIX", "DOCSRS_PREFIX"),
            ("CRATESFYI_DATABASE_URL", "DOCSRS_DATABASE_URL"),
            ("CRATESFYI_GITHUB_ACCESSTOKEN", "DOCSRS_GITHUB_ACCESSTOKEN"),
            ("CRATESFYI_RUSTWIDE_WORKSPACE", "DOCSRS_RUSTWIDE_WORKSPACE"),
            ("DOCS_RS_DOCKER", "DOCSRS_DOCKER"),
            ("DOCS_RS_LOCAL_DOCKER_IMAGE", "DOCSRS_DOCKER_IMAGE"),
            ("DOCS_RS_BUILD_CPU_LIMIT", "DOCSRS_BUILD_CPU_LIMIT"),
        ];
        for (old_var, new_var) in old_vars {
            if std::env::var(old_var).is_ok() {
                bail!(
                    "env variable {} is no longer accepted; use {} instead",
                    old_var,
                    new_var
                );
            }
        }

        let prefix: PathBuf = require_env("DOCSRS_PREFIX")?;
        let temp_dir = prefix.join("tmp");

        Ok(ConfigBuilder::default()
            .build_attempts(env("DOCSRS_BUILD_ATTEMPTS", 5u16)?)
            .delay_between_build_attempts(Duration::from_secs(env::<u64>(
                "DOCSRS_DELAY_BETWEEN_BUILD_ATTEMPTS",
                60,
            )?))
            .crates_io_api_call_retries(env("DOCSRS_CRATESIO_API_CALL_RETRIES", 3u32)?)
            .registry_index_path(env("REGISTRY_INDEX_PATH", prefix.join("crates.io-index"))?)
            .registry_url(maybe_env("REGISTRY_URL")?)
            .registry_api_host(env(
                "DOCSRS_REGISTRY_API_HOST",
                "https://crates.io".parse().unwrap(),
            )?)
            .opentelemetry_endpoint(maybe_env("OTEL_EXPORTER_OTLP_ENDPOINT")?)
            .prefix(prefix.clone())
            .database_url(require_env("DOCSRS_DATABASE_URL")?)
            .max_pool_size(env("DOCSRS_MAX_POOL_SIZE", 90u32)?)
            .min_pool_idle(env("DOCSRS_MIN_POOL_IDLE", 10u32)?)
            .cratesio_token(maybe_env("DOCSRS_CRATESIO_TOKEN")?)
            // LOL HTML only uses as much memory as the size of the start tag!
            // https://github.com/rust-lang/docs.rs/pull/930#issuecomment-667729380
            .max_parse_memory(env("DOCSRS_MAX_PARSE_MEMORY", 5 * 1024 * 1024)?)
            .registry_gc_interval(env("DOCSRS_REGISTRY_GC_INTERVAL", 60 * 60)?)
            .render_threads(env("DOCSRS_RENDER_THREADS", num_cpus::get())?)
            .request_timeout(maybe_env::<u64>("DOCSRS_REQUEST_TIMEOUT")?.map(Duration::from_secs))
            .report_request_timeouts(env("DOCSRS_REPORT_REQUEST_TIMEOUTS", false)?)
            .random_crate_search_view_size(env("DOCSRS_RANDOM_CRATE_SEARCH_VIEW_SIZE", 500)?)
            .csp_report_only(env("DOCSRS_CSP_REPORT_ONLY", false)?)
            .cache_control_stale_while_revalidate(maybe_env(
                "CACHE_CONTROL_STALE_WHILE_REVALIDATE",
            )?)
            .cache_invalidatable_responses(env("DOCSRS_CACHE_INVALIDATEABLE_RESPONSES", true)?)
            .fastly_api_host(env(
                "DOCSRS_FASTLY_API_HOST",
                "https://api.fastly.com".parse().unwrap(),
            )?)
            .fastly_api_token(maybe_env("DOCSRS_FASTLY_API_TOKEN")?)
            .fastly_service_sid(maybe_env("DOCSRS_FASTLY_SERVICE_SID_WEB")?)
            .compiler_metrics_collection_path(maybe_env("DOCSRS_COMPILER_METRICS_PATH")?)
            .temp_dir(temp_dir)
            .rustwide_workspace(env(
                "DOCSRS_RUSTWIDE_WORKSPACE",
                PathBuf::from(".workspace"),
            )?)
            .inside_docker(env("DOCSRS_DOCKER", false)?)
            .docker_image(
                maybe_env("DOCSRS_LOCAL_DOCKER_IMAGE")?.or(maybe_env("DOCSRS_DOCKER_IMAGE")?),
            )
            .build_cpu_limit(maybe_env("DOCSRS_BUILD_CPU_LIMIT")?)
            .build_default_memory_limit(maybe_env("DOCSRS_BUILD_DEFAULT_MEMORY_LIMIT")?)
            .include_default_targets(env("DOCSRS_INCLUDE_DEFAULT_TARGETS", true)?)
            .disable_memory_limit(env("DOCSRS_DISABLE_MEMORY_LIMIT", false)?)
            .build_workspace_reinitialization_interval(Duration::from_secs(env(
                "DOCSRS_BUILD_WORKSPACE_REINITIALIZATION_INTERVAL",
                86400,
            )?))
            .max_queued_rebuilds(maybe_env("DOCSRS_MAX_QUEUED_REBUILDS")?))
    }
}
