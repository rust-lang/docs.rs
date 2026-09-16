use anyhow::Result;
use docs_rs_config::AppConfig;
use docs_rs_env_vars::{env, maybe_env, require_env};
use docs_rs_types::Duration;
use std::path::PathBuf;

#[derive(Debug)]
pub struct Config {
    pub registry_index_path: PathBuf,
    pub registry_url: Option<String>,

    /// How long to wait between registry checks
    pub delay_between_registry_fetches: Duration,

    // Time between 'git gc --auto' calls in seconds
    pub registry_gc_interval: Duration,

    // automatic rebuild configuration
    pub max_queued_rebuilds: Option<u16>,

    /// Maximum time to wait for queue row locks when deleting crates/releases.
    pub delete_lock_timeout: Duration,

    pub repository: docs_rs_repository_stats::Config,
}

impl AppConfig for Config {
    fn from_environment() -> Result<Self> {
        let prefix: PathBuf = require_env("DOCSRS_PREFIX")?;
        Ok(Self {
            registry_index_path: env("REGISTRY_INDEX_PATH", prefix.join("crates.io-index"))?,
            registry_url: maybe_env("REGISTRY_URL")?,
            delay_between_registry_fetches: env(
                "DOCSRS_DELAY_BETWEEN_REGISTRY_FETCHES",
                Duration::from_mins(1),
            )?,
            registry_gc_interval: env("DOCSRS_REGISTRY_GC_INTERVAL", Duration::from_hours(1))?,
            max_queued_rebuilds: maybe_env("DOCSRS_MAX_QUEUED_REBUILDS")?,
            delete_lock_timeout: env(
                "DOCSRS_DELETE_LOCK_TIMEOUT_SECONDS",
                Duration::from_mins(20),
            )?,
            repository: docs_rs_repository_stats::Config::from_environment()?,
        })
    }
}
