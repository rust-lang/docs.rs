use crate::storage::StorageKind;
use anyhow::{Context as _, Result, bail};
use docs_rs_env_vars::{env, maybe_env, require_env};
use std::{
    io,
    path::{self, Path, PathBuf},
    time::Duration,
};

#[derive(Debug, derive_builder::Builder)]
#[builder(pattern = "owned")]
pub struct Config {
    pub prefix: PathBuf,
    pub registry_index_path: PathBuf,
    pub registry_url: Option<String>,

    /// How long to wait between registry checks
    pub(crate) delay_between_registry_fetches: Duration,

    // Database connection params
    pub(crate) database_url: String,
    pub(crate) max_pool_size: u32,
    pub(crate) min_pool_idle: u32,

    // Storage params
    pub(crate) storage_backend: StorageKind,

    // AWS SDK configuration
    pub(crate) aws_sdk_max_retries: u32,

    // S3 params
    pub(crate) s3_bucket: String,
    pub(crate) s3_region: String,
    pub(crate) s3_endpoint: Option<String>,

    // DO NOT CONFIGURE THIS THROUGH AN ENVIRONMENT VARIABLE!
    // Accidentally turning this on outside of the test suite might cause data loss in the
    // production environment.
    #[cfg(test)]
    #[builder(default)]
    pub(crate) s3_bucket_is_temporary: bool,

    // Github authentication
    pub(crate) github_accesstoken: Option<String>,
    pub(crate) github_updater_min_rate_limit: u32,

    // GitLab authentication
    pub(crate) gitlab_accesstoken: Option<String>,

    // Access token for APIs for crates.io (careful: use
    // constant_time_eq for comparisons!)
    pub(crate) cratesio_token: Option<String>,

    // request timeout in seconds
    pub(crate) request_timeout: Option<Duration>,
    pub(crate) report_request_timeouts: bool,

    // Max size of the files served by the docs.rs frontend
    pub(crate) max_file_size: usize,
    pub(crate) max_file_size_html: usize,
    // The most memory that can be used to parse an HTML file
    pub(crate) max_parse_memory: usize,
    // Time between 'git gc --auto' calls in seconds
    pub(crate) registry_gc_interval: u64,

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

    // where do we want to store the locally cached index files
    // for the remote archives?
    pub(crate) local_archive_cache_path: PathBuf,

    // expected number of entries in the local archive cache.
    // Makes server restarts faster by preallocating some data structures.
    // General numbers (as of 2025-12):
    // * we have ~1.5 mio releases with archive storage (and 400k without)
    // * each release has on average 2 archive files (rustdoc, source)
    // so, over all, 3 mio archive index files in S3.
    //
    // While due to crawlers we will download _all_ of them over time, the old
    // metric "releases accessed in the last 10 minutes" was around 50k, if I
    // recall correctly.
    // We're using a local DashMap to store some locks for these indexes,
    // and we already know in advance we need these 50k entries.
    // So we can preallocate the DashMap with this number to avoid resizes.
    pub(crate) local_archive_cache_expected_count: usize,

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

    // automatic rebuild configuration
    pub(crate) max_queued_rebuilds: Option<u16>,

    pub(crate) fastly: docs_rs_fastly::Config,
    pub(crate) opentelemetry: docs_rs_opentelemetry::Config,
    pub(crate) registry_api: docs_rs_registry_api::Config,
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
            .delay_between_registry_fetches(Duration::from_secs(env::<u64>(
                "DOCSRS_DELAY_BETWEEN_REGISTRY_FETCHES",
                60,
            )?))
            .registry_index_path(env("REGISTRY_INDEX_PATH", prefix.join("crates.io-index"))?)
            .registry_url(maybe_env("REGISTRY_URL")?)
            .prefix(prefix.clone())
            .database_url(require_env("DOCSRS_DATABASE_URL")?)
            .max_pool_size(env("DOCSRS_MAX_POOL_SIZE", 90u32)?)
            .min_pool_idle(env("DOCSRS_MIN_POOL_IDLE", 10u32)?)
            .storage_backend(env("DOCSRS_STORAGE_BACKEND", StorageKind::Database)?)
            .aws_sdk_max_retries(env("DOCSRS_AWS_SDK_MAX_RETRIES", 6u32)?)
            .s3_bucket(env("DOCSRS_S3_BUCKET", "rust-docs-rs".to_string())?)
            .s3_region(env("S3_REGION", "us-west-1".to_string())?)
            .s3_endpoint(maybe_env("S3_ENDPOINT")?)
            .github_accesstoken(maybe_env("DOCSRS_GITHUB_ACCESSTOKEN")?)
            .github_updater_min_rate_limit(env("DOCSRS_GITHUB_UPDATER_MIN_RATE_LIMIT", 2500u32)?)
            .gitlab_accesstoken(maybe_env("DOCSRS_GITLAB_ACCESSTOKEN")?)
            .cratesio_token(maybe_env("DOCSRS_CRATESIO_TOKEN")?)
            .max_file_size(env("DOCSRS_MAX_FILE_SIZE", 50 * 1024 * 1024)?)
            .max_file_size_html(env("DOCSRS_MAX_FILE_SIZE_HTML", 50 * 1024 * 1024)?)
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
            .local_archive_cache_path(ensure_absolute_path(env(
                "DOCSRS_ARCHIVE_INDEX_CACHE_PATH",
                prefix.join("archive_cache"),
            )?)?)
            .local_archive_cache_expected_count(env(
                "DOCSRS_ARCHIVE_INDEX_EXPECTED_COUNT",
                100_000usize,
            )?)
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
            .max_queued_rebuilds(maybe_env("DOCSRS_MAX_QUEUED_REBUILDS")?)
            .fastly(
                docs_rs_fastly::Config::from_environment()
                    .context("error reading fastly config from environment")?,
            )
            .opentelemetry(docs_rs_opentelemetry::Config::from_environment()?)
            .registry_api(docs_rs_registry_api::Config::from_environment()?))
    }

    pub fn max_file_size_for(&self, path: impl AsRef<Path>) -> usize {
        static HTML: &str = "html";

        if let Some(ext) = path.as_ref().extension()
            && ext == HTML
        {
            self.max_file_size_html
        } else {
            self.max_file_size
        }
    }
}

fn ensure_absolute_path(path: PathBuf) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(path::absolute(&path)?)
    }
}
