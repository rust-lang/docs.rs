use crate::{cdn::CdnKind, storage::StorageKind};
use anyhow::{anyhow, bail, Context, Result};
use std::{env::VarError, error::Error, path::PathBuf, str::FromStr, time::Duration};
use tracing::trace;
use url::Url;

#[derive(Debug)]
pub struct Config {
    pub prefix: PathBuf,
    pub registry_index_path: PathBuf,
    pub registry_url: Option<String>,
    pub registry_api_host: Url,

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
    #[cfg(test)]
    pub(crate) s3_bucket_is_temporary: bool,

    // CloudFront domain which we can access
    // public S3 files through
    pub(crate) s3_static_root_path: String,

    // Github authentication
    pub(crate) github_accesstoken: Option<String>,
    pub(crate) github_updater_min_rate_limit: u32,

    // Gitlab authentication
    pub(crate) gitlab_accesstoken: Option<String>,

    // Access token for APIs for crates.io (careful: use
    // constant_time_eq for comparisons!)
    pub(crate) cratesio_token: Option<String>,

    // amount of retries for external API calls, mostly crates.io
    pub crates_io_api_call_retries: u32,

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

    pub(crate) cdn_backend: CdnKind,

    /// The maximum age of a queued invalidation request before it is
    /// considered too old and we fall back to a full purge of the
    /// distributions.
    pub(crate) cdn_max_queued_age: Duration,

    // CloudFront distribution ID for the web server.
    // Will be used for invalidation-requests.
    pub cloudfront_distribution_id_web: Option<String>,
    /// same for the `static.docs.rs` distribution
    pub cloudfront_distribution_id_static: Option<String>,
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
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let old_vars = [
            ("CRATESFYI_PREFIX", "DOCSRS_PREFIX"),
            ("CRATESFYI_DATABASE_URL", "DOCSRS_DATABASE_URL"),
            ("CRATESFYI_GITHUB_ACCESSTOKEN", "DOCSRS_GITHUB_ACCESSTOKEN"),
            ("CRATESFYI_RUSTWIDE_WORKSPACE", "DOCSRS_RUSTWIDE_WORKSPACE"),
            ("DOCS_RS_DOCKER", "DOCSRS_DOCKER"),
            ("DOCS_RS_LOCAL_DOCKER_IMAGE", "DOCSRS_DOCKER_IMAGE"),
            ("DOCS_RS_BULID_CPU_LIMIT", "DOCSRS_BULID_CPU_LIMIT"),
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

        Ok(Self {
            build_attempts: env("DOCSRS_BUILD_ATTEMPTS", 5)?,
            delay_between_build_attempts: Duration::from_secs(env::<u64>(
                "DOCSRS_DELAY_BETWEEN_BUILD_ATTEMPTS",
                60,
            )?),
            delay_between_registry_fetches: Duration::from_secs(env::<u64>(
                "DOCSRS_DELAY_BETWEEN_REGISTRY_FETCHES",
                60,
            )?),

            crates_io_api_call_retries: env("DOCSRS_CRATESIO_API_CALL_RETRIES", 3)?,

            registry_index_path: env("REGISTRY_INDEX_PATH", prefix.join("crates.io-index"))?,
            registry_url: maybe_env("REGISTRY_URL")?,
            registry_api_host: env(
                "DOCSRS_REGISTRY_API_HOST",
                "https://crates.io".parse().unwrap(),
            )?,
            prefix: prefix.clone(),

            database_url: require_env("DOCSRS_DATABASE_URL")?,
            max_pool_size: env("DOCSRS_MAX_POOL_SIZE", 90)?,
            min_pool_idle: env("DOCSRS_MIN_POOL_IDLE", 10)?,

            storage_backend: env("DOCSRS_STORAGE_BACKEND", StorageKind::Database)?,

            aws_sdk_max_retries: env("DOCSRS_AWS_SDK_MAX_RETRIES", 6)?,

            s3_bucket: env("DOCSRS_S3_BUCKET", "rust-docs-rs".to_string())?,
            s3_region: env("S3_REGION", "us-west-1".to_string())?,
            s3_endpoint: maybe_env("S3_ENDPOINT")?,
            // DO NOT CONFIGURE THIS THROUGH AN ENVIRONMENT VARIABLE!
            // Accidentally turning this on outside of the test suite might cause data loss in the
            // production environment.
            #[cfg(test)]
            s3_bucket_is_temporary: false,

            s3_static_root_path: env(
                "DOCSRS_S3_STATIC_ROOT_PATH",
                "https://static.docs.rs".to_string(),
            )?,

            github_accesstoken: maybe_env("DOCSRS_GITHUB_ACCESSTOKEN")?,
            github_updater_min_rate_limit: env("DOCSRS_GITHUB_UPDATER_MIN_RATE_LIMIT", 2500)?,

            gitlab_accesstoken: maybe_env("DOCSRS_GITLAB_ACCESSTOKEN")?,

            cratesio_token: maybe_env("DOCSRS_CRATESIO_TOKEN")?,

            max_file_size: env("DOCSRS_MAX_FILE_SIZE", 50 * 1024 * 1024)?,
            max_file_size_html: env("DOCSRS_MAX_FILE_SIZE_HTML", 50 * 1024 * 1024)?,
            // LOL HTML only uses as much memory as the size of the start tag!
            // https://github.com/rust-lang/docs.rs/pull/930#issuecomment-667729380
            max_parse_memory: env("DOCSRS_MAX_PARSE_MEMORY", 5 * 1024 * 1024)?,
            registry_gc_interval: env("DOCSRS_REGISTRY_GC_INTERVAL", 60 * 60)?,
            render_threads: env("DOCSRS_RENDER_THREADS", num_cpus::get())?,
            request_timeout: maybe_env::<u64>("DOCSRS_REQUEST_TIMEOUT")?.map(Duration::from_secs),
            report_request_timeouts: env("DOCSRS_REPORT_REQUEST_TIMEOUTS", false)?,

            random_crate_search_view_size: env("DOCSRS_RANDOM_CRATE_SEARCH_VIEW_SIZE", 500)?,

            csp_report_only: env("DOCSRS_CSP_REPORT_ONLY", false)?,

            cache_control_stale_while_revalidate: maybe_env(
                "CACHE_CONTROL_STALE_WHILE_REVALIDATE",
            )?,

            cache_invalidatable_responses: env("DOCSRS_CACHE_INVALIDATEABLE_RESPONSES", true)?,

            cdn_backend: env("DOCSRS_CDN_BACKEND", CdnKind::Dummy)?,
            cdn_max_queued_age: Duration::from_secs(env("DOCSRS_CDN_MAX_QUEUED_AGE", 3600)?),

            cloudfront_distribution_id_web: maybe_env("CLOUDFRONT_DISTRIBUTION_ID_WEB")?,
            cloudfront_distribution_id_static: maybe_env("CLOUDFRONT_DISTRIBUTION_ID_STATIC")?,

            local_archive_cache_path: env(
                "DOCSRS_ARCHIVE_INDEX_CACHE_PATH",
                prefix.join("archive_cache"),
            )?,

            temp_dir,

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

fn env<T>(var: &str, default: T) -> Result<T>
where
    T: FromStr,
    T::Err: Error + Send + Sync + 'static,
{
    Ok(maybe_env(var)?.unwrap_or(default))
}

fn require_env<T>(var: &str) -> Result<T>
where
    T: FromStr,
    <T as FromStr>::Err: Error + Send + Sync + 'static,
{
    maybe_env(var)?.with_context(|| anyhow!("configuration variable {} is missing", var))
}

fn maybe_env<T>(var: &str) -> Result<Option<T>>
where
    T: FromStr,
    T::Err: Error + Send + Sync + 'static,
{
    match std::env::var(var) {
        Ok(content) => Ok(content
            .parse::<T>()
            .map(Some)
            .with_context(|| format!("failed to parse configuration variable {var}"))?),
        Err(VarError::NotPresent) => {
            trace!("optional configuration variable {} is not set", var);
            Ok(None)
        }
        Err(VarError::NotUnicode(_)) => Err(anyhow!("configuration variable {} is not UTF-8", var)),
    }
}
