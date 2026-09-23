use anyhow::Result;
use docs_rs_config::AppConfig;
use docs_rs_env_vars::maybe_env;
use docs_rs_types::Duration;
use std::sync::LazyLock;
use url::Url;

static FETCH_URL: LazyLock<Url> = LazyLock::new(|| {
    Url::parse("https://rust-lang.github.io/std-replacement-data/all.json").unwrap()
});

/// Configuration for the standard-library replacement client.
#[derive(Debug, bon::Builder)]
#[builder(on(_, overwritable))]
pub struct Config {
    /// URL of the complete replacement dataset.
    #[builder(default = FETCH_URL.clone())]
    pub url: Url,

    /// Maximum number of retries for transient HTTP failures.
    #[builder(default = 3)]
    pub max_retries: u32,

    /// Default TTL for the cache, if we can't read it from the headers.
    #[builder(default = Duration::from_mins(10))]
    pub cache_default_ttl: Duration,
}

impl AppConfig for Config {
    fn from_environment() -> Result<Self> {
        Ok(Self::builder()
            .maybe_url(maybe_env("DOCSRS_STD_REPLACEMENTS_URL")?)
            .maybe_max_retries(maybe_env("DOCSRS_STD_REPLACEMENTS_RETRIES")?)
            .maybe_cache_default_ttl(maybe_env("DOCSRS_STD_REPLACEMENTS_CACHE_DEFAULT_TTL")?)
            .build())
    }
}
