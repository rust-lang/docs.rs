use docs_rs_env_vars::env;
use url::Url;

#[derive(Debug)]
pub struct Config {
    pub registry_api_host: Url,

    // amount of retries for external API calls, mostly crates.io
    pub crates_io_api_call_retries: u32,
}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {
        Ok(Self {
            crates_io_api_call_retries: env("DOCSRS_CRATESIO_API_CALL_RETRIES", 3u32)?,
            registry_api_host: env(
                "DOCSRS_REGISTRY_API_HOST",
                "https://crates.io".parse().unwrap(),
            )?,
        })
    }
}
