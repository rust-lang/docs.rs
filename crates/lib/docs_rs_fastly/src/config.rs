use docs_rs_env_vars::{env, maybe_env};
use url::Url;

const FASTLY_API_HOST: &str = "https://api.fastly.com";

#[derive(Debug)]
pub struct Config {
    /// Fastly API host, typically only overwritten for testing
    pub api_host: Url,

    /// Fastly API token for purging the services below.
    pub api_token: Option<String>,

    /// fastly service SID for the main domain
    pub service_sid: Option<String>,
}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {
        Ok(Self {
            api_host: env("DOCSRS_FASTLY_API_HOST", FASTLY_API_HOST.parse().unwrap())?,
            api_token: maybe_env("DOCSRS_FASTLY_API_TOKEN")?,
            service_sid: maybe_env("DOCSRS_FASTLY_SERVICE_SID_WEB")?,
        })
    }

    /// test config
    /// assumes we're using the mock CDN, but generates a config where
    /// `is_valid` is true.`
    #[cfg(any(test, feature = "testing"))]
    pub fn test_config() -> Self {
        let cfg = Self {
            api_host: FASTLY_API_HOST.parse().unwrap(),
            api_token: Some("some_token".into()),
            service_sid: Some("some_sid".into()),
        };

        debug_assert!(cfg.is_valid());

        cfg
    }

    pub fn is_valid(&self) -> bool {
        self.api_token.is_some() && self.service_sid.is_some()
    }
}
