use anyhow::Result;
use docs_rs_config::AppConfig;
use docs_rs_env_vars::maybe_env;
use tracing::warn;
use url::Url;

#[derive(Debug, bon::Builder)]
/// Configuration for [`crate::RegistryApi`].
pub struct Config {
    /// Base URL of the sparse registry index, including its `sparse+` scheme.
    ///
    /// Defaults to the crates.io sparse index.
    #[builder(default =  crates_index::sparse::URL.parse().unwrap())]
    pub sparse_index_host: Url,

    /// Maximum number of retries for transient registry HTTP failures.
    #[builder(default = 3)]
    pub crates_io_api_call_retries: u32,
}

impl AppConfig for Config {
    fn from_environment() -> Result<Self> {
        if maybe_env::<String>("DOCSRS_REGISTRY_API_HOST")?.is_some() {
            warn!("legacy config for registry api host. will be ignored.")
        }

        Ok(Self::builder()
            .maybe_crates_io_api_call_retries(maybe_env("DOCSRS_CRATESIO_API_CALL_RETRIES")?)
            .maybe_sparse_index_host(maybe_env("DOCSRS_SPARSE_INDEX_HOST")?)
            .build())
    }
}
