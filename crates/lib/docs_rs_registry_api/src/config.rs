use docs_rs_env_vars::maybe_env;
use url::Url;

#[derive(Debug)]
pub struct Config {
    pub registry_api_host: Url,

    // amount of retries for external API calls, mostly crates.io
    pub crates_io_api_call_retries: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            crates_io_api_call_retries: 3,
            registry_api_host: "https://crates.io".parse().unwrap(),
        }
    }
}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {
        let mut config = Self::default();

        if let Some(api_call_retries) = maybe_env::<u32>("DOCSRS_CRATESIO_API_CALL_RETRIES")? {
            config.crates_io_api_call_retries = api_call_retries;
        }

        if let Some(registry_api_host) = maybe_env::<Url>("DOCSRS_REGISTRY_API_HOST")? {
            config.registry_api_host = registry_api_host;
        }

        Ok(config)
    }
}
