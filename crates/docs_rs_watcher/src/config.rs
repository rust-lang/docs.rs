use docs_rs_env_vars::{env, maybe_env, require_env};
use std::{path::PathBuf, time::Duration};
use url::Url;

#[derive(Debug)]
pub struct Config {
    pub(crate) registry_index_path: PathBuf,
    pub(crate) registry_url: Option<String>,
    pub(crate) registry_api_host: Url,

    /// How long to wait between registry checks
    pub(crate) delay_between_registry_fetches: Duration,

    // Time between 'git gc --auto' calls in seconds
    pub(crate) registry_gc_interval: u64,
}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {
        let prefix: PathBuf = require_env("DOCSRS_PREFIX")?;
        Ok(Self {
            registry_index_path: env("REGISTRY_INDEX_PATH", prefix.join("crates.io-index"))?,
            registry_url: maybe_env("REGISTRY_URL")?,
            registry_api_host: env(
                "DOCSRS_REGISTRY_API_HOST",
                "https://crates.io".parse().unwrap(),
            )?,
            delay_between_registry_fetches: Duration::from_secs(env::<u64>(
                "DOCSRS_DELAY_BETWEEN_REGISTRY_FETCHES",
                60,
            )?),
            registry_gc_interval: env("DOCSRS_REGISTRY_GC_INTERVAL", 60 * 60)?,
        })
    }
}
