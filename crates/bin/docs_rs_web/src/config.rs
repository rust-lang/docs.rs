use docs_rs_env_vars::{env, maybe_env};
use url::Url;

#[derive(Debug)]
pub struct Config {}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {
        Ok(Self {
            // api_host: env(
            //     "DOCSRS_FASTLY_API_HOST",
            //     "https://api.fastly.com".parse().unwrap(),
            // )?,
            // api_token: maybe_env("DOCSRS_FASTLY_API_TOKEN")?,
            // service_sid: maybe_env("DOCSRS_FASTLY_SERVICE_SID_WEB")?,
        })
    }
}
