use anyhow::Result;
use docs_rs_env_vars::maybe_env;
use url::Url;

#[derive(Debug, bon::Builder)]
pub struct Config {
    #[builder(default =  "https://crates.io".parse().unwrap())]
    pub registry_api_host: Url,

    // amount of retries for external API calls, mostly crates.io
    #[builder(default = 3)]
    pub crates_io_api_call_retries: u32,
}

impl Config {
    pub fn from_environment() -> Result<Self> {
        Ok(Self::builder()
            .maybe_crates_io_api_call_retries(maybe_env("DOCSRS_CRATESIO_API_CALL_RETRIES")?)
            .maybe_registry_api_host(maybe_env("DOCSRS_REGISTRY_API_HOST")?)
            .build())
    }
}
