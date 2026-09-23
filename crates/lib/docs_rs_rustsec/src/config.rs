use anyhow::Result;
use docs_rs_config::AppConfig;
use docs_rs_env_vars::maybe_env;
use docs_rs_types::Duration;
use url::Url;

/// Configuration for [`crate::RustsecClient`].
#[derive(Debug, bon::Builder)]
#[builder(on(_, overwritable))]
pub struct Config {
    /// RustSec site root URL, without a path prefix, query, or fragment.
    #[builder(default = "https://rustsec.org/".parse().unwrap())]
    pub base_url: Url,

    /// Maximum number of retries for transient HTTP failures.
    #[builder(default = 3)]
    pub max_retries: u32,

    /// Maximum number of cached crate results, including missing feeds.
    #[builder(default = 100_000)]
    pub cache_capacity: u64,

    /// Default TTL for the cache, if we can't read it from the headers.
    /// ( also, for caching 404s)
    #[builder(default = Duration::from_mins(10))]
    pub cache_default_ttl: Duration,
}

impl AppConfig for Config {
    fn from_environment() -> Result<Self> {
        Ok(Self::builder()
            .maybe_base_url(maybe_env("DOCSRS_RUSTSEC_BASE_URL")?)
            .maybe_max_retries(maybe_env("DOCSRS_RUSTSEC_RETRIES")?)
            .maybe_cache_capacity(maybe_env("DOCSRS_RUSTSEC_CACHE_CAPACITY")?)
            .maybe_cache_default_ttl(maybe_env("DOCSRS_RUSTSEC_CACHE_DEFAULT_TTL")?)
            .build())
    }
}
