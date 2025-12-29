use anyhow::{Context as _, Result, bail};
use docs_rs_context::Config as NewConfig;
use docs_rs_env_vars::{env, maybe_env, require_env};
use std::{path::PathBuf, sync::Arc, time::Duration};

#[derive(Debug, derive_builder::Builder)]
#[builder(pattern = "owned")]
pub struct Config {
    pub prefix: PathBuf,

    // Access token for APIs for crates.io (careful: use
    // constant_time_eq for comparisons!)
    pub(crate) cratesio_token: Option<String>,

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

    pub(crate) fastly: Arc<docs_rs_fastly::Config>,
    pub(crate) opentelemetry: Arc<docs_rs_opentelemetry::Config>,
    pub(crate) registry_api: Arc<docs_rs_registry_api::Config>,
    pub(crate) database: Arc<docs_rs_database::Config>,
    pub(crate) repository_stats: Arc<docs_rs_repository_stats::Config>,
    pub(crate) storage: Arc<docs_rs_storage::Config>,
    pub(crate) build_queue: Arc<docs_rs_build_queue::Config>,
    pub(crate) build_limits: Arc<docs_rs_build_limits::Config>,
    pub builder: Arc<docs_rs_builder::Config>,
    pub watcher: Arc<docs_rs_watcher::Config>,
}

impl From<&Config> for NewConfig {
    fn from(value: &Config) -> Self {
        Self {
            build_queue: Some(value.build_queue.clone()),
            database: Some(value.database.clone()),
            storage: Some(value.storage.clone()),
            registry_api: Some(value.registry_api.clone()),
            cdn: value.fastly.is_valid().then(|| value.fastly.clone()),
            repository_stats: Some(value.repository_stats.clone()),
        }
    }
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

        Ok(ConfigBuilder::default()
            .prefix(prefix.clone())
            .cratesio_token(maybe_env("DOCSRS_CRATESIO_TOKEN")?)
            // LOL HTML only uses as much memory as the size of the start tag!
            // https://github.com/rust-lang/docs.rs/pull/930#issuecomment-667729380
            .max_parse_memory(env("DOCSRS_MAX_PARSE_MEMORY", 5 * 1024 * 1024)?)
            .render_threads(env("DOCSRS_RENDER_THREADS", num_cpus::get())?)
            .request_timeout(maybe_env::<u64>("DOCSRS_REQUEST_TIMEOUT")?.map(Duration::from_secs))
            .report_request_timeouts(env("DOCSRS_REPORT_REQUEST_TIMEOUTS", false)?)
            .random_crate_search_view_size(env("DOCSRS_RANDOM_CRATE_SEARCH_VIEW_SIZE", 500)?)
            .csp_report_only(env("DOCSRS_CSP_REPORT_ONLY", false)?)
            .cache_control_stale_while_revalidate(maybe_env(
                "CACHE_CONTROL_STALE_WHILE_REVALIDATE",
            )?)
            .cache_invalidatable_responses(env("DOCSRS_CACHE_INVALIDATEABLE_RESPONSES", true)?)
            .fastly(Arc::new(
                docs_rs_fastly::Config::from_environment()
                    .context("error reading fastly config from environment")?,
            ))
            .opentelemetry(Arc::new(docs_rs_opentelemetry::Config::from_environment()?))
            .registry_api(Arc::new(docs_rs_registry_api::Config::from_environment()?))
            .database(Arc::new(docs_rs_database::Config::from_environment()?))
            .repository_stats(Arc::new(
                docs_rs_repository_stats::Config::from_environment()?,
            ))
            .storage(Arc::new(docs_rs_storage::Config::from_environment()?))
            .build_queue(Arc::new(docs_rs_build_queue::Config::from_environment()?))
            .build_limits(Arc::new(docs_rs_build_limits::Config::from_environment()?))
            .builder(Arc::new(docs_rs_builder::Config::from_environment()?))
            .watcher(Arc::new(docs_rs_watcher::Config::from_environment()?)))
    }
}
