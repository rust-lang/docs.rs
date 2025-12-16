use docs_rs_env_vars::{env, maybe_env};
use url::Url;

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
            api_host: env(
                "DOCSRS_FASTLY_API_HOST",
                "https://api.fastly.com".parse().unwrap(),
            )?,
            api_token: maybe_env("DOCSRS_FASTLY_API_TOKEN")?,
            service_sid: maybe_env("DOCSRS_FASTLY_SERVICE_SID_WEB")?,
        })
    }

    pub fn is_valid(&self) -> bool {
        self.api_token.is_some() && self.service_sid.is_some()
    }
}
