use docs_rs_env_vars::{env, maybe_env, require_env};
use std::{path::PathBuf, time::Duration};
use url::Url;

#[derive(Debug)]
pub struct Config {
    pub registry_index_path: PathBuf,
    pub registry_url: Option<String>,
    pub registry_api_host: Url,

    /// How long to wait between registry checks
    pub delay_between_registry_fetches: Duration,

    // Time between 'git gc --auto' calls in seconds
    pub registry_gc_interval: u64,

    // Github authentication
    pub github_accesstoken: Option<String>,
    pub github_updater_min_rate_limit: u32,

    // GitLab authentication
    pub gitlab_accesstoken: Option<String>,
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
            github_accesstoken: maybe_env("DOCSRS_GITHUB_ACCESSTOKEN")?,
            github_updater_min_rate_limit: env("DOCSRS_GITHUB_UPDATER_MIN_RATE_LIMIT", 2500u32)?,
            gitlab_accesstoken: maybe_env("DOCSRS_GITLAB_ACCESSTOKEN")?,
        })
    }
}
