use anyhow::Result;
use docs_rs_config::AppConfig;
use docs_rs_env_vars::{env, maybe_env, require_env};
use std::{path::PathBuf, time::Duration};
use url::Url;

#[derive(Debug)]
pub struct SqsConfig {
    pub queue_url: Url,
    pub region: String,
    pub endpoint_url: Url,
    pub max_retries: u32,
    /// temporary, to switch between the sources for the index (git index vs SQS)
    pub active: bool,
}

impl AppConfig for SqsConfig {
    fn from_environment() -> Result<Self> {
        let prefix: PathBuf = require_env("DOCSRS_PREFIX")?;
        Ok(Self {
            queue_url: mandatory_env("DOCSRS_SQS_QUEUE_URL")?,
            region: mandatory_env("DOCSRS_SQS_REGION")?,
            endpoint_url: mandatory_env("DOCSRS_SQS_ENDPOINT_URL")?,
            active: env("DOCSRS_SQS_ACTIVE", false)?,
            max_retries: env("DOCSRS_MAX_RETRIES", 6u32)?,
        })
    }
}

#[derive(Debug)]
pub struct Config {
    /// registry watching config. Also used for database-synchonize
    pub registry_index_path: PathBuf,
    pub registry_url: Option<String>,
    /// How long to wait between registry checks
    pub delay_between_registry_fetches: Duration,
    // Time between 'git gc --auto' calls in seconds
    pub registry_gc_interval: u64,

    // automatic rebuild configuration
    pub max_queued_rebuilds: Option<u16>,

    /// Maximum time to wait for queue row locks when deleting crates/releases.
    pub delete_lock_timeout: Duration,

    pub crates_io_events: Option<SqsConfig>,

    pub repository: docs_rs_repository_stats::Config,
}

impl AppConfig for Config {
    fn from_environment() -> Result<Self> {
        let prefix: PathBuf = require_env("DOCSRS_PREFIX")?;
        Ok(Self {
            registry_index_path: env("REGISTRY_INDEX_PATH", prefix.join("crates.io-index"))?,
            registry_url: maybe_env("REGISTRY_URL")?,

            crates_io_events: Arc::new(SqsConfig::from_environment()?),

            delay_between_registry_fetches: Duration::from_secs(env::<u64>(
                "DOCSRS_DELAY_BETWEEN_REGISTRY_FETCHES",
                60,
            )?),
            registry_gc_interval: env("DOCSRS_REGISTRY_GC_INTERVAL", 60 * 60)?,
            max_queued_rebuilds: maybe_env("DOCSRS_MAX_QUEUED_REBUILDS")?,
            delete_lock_timeout: Duration::from_secs(env::<u64>(
                "DOCSRS_DELETE_LOCK_TIMEOUT_SECONDS",
                20 * 60,
            )?),
            repository: docs_rs_repository_stats::Config::from_environment()?,
        })
    }

    #[cfg(test)]
    fn test_config() -> Result<Self> {
        let mut config = Self::from_environment()?;
        config.crates_io_events = None;
        Ok(config)
    }
}
