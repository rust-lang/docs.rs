use anyhow::Result;
use docs_rs_config::AppConfig;
use docs_rs_env_vars::maybe_env;
use http::HeaderValue;
use tracing::warn;
use url::Url;

#[derive(Debug, bon::Builder)]
/// Configuration for [`crate::RegistryApi`].
pub struct Config {
    /// Optional authentication token, sent in the X-Docsrs-Authorization header for registry API requests.
    ///
    /// Crates.io itself ignores the token on the endpoints we use,
    /// we will check this header value on the NgWAF / CDN level to bypass the normal rate limits.
    pub api_token: Option<HeaderValue>,

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

        let token = maybe_env::<HeaderValue>("DOCSRS_REGISTRY_API_TOKEN")?.map(|mut token| {
            token.set_sensitive(true);
            token
        });

        Ok(Self::builder()
            .maybe_api_token(token)
            .maybe_crates_io_api_call_retries(maybe_env("DOCSRS_CRATESIO_API_CALL_RETRIES")?)
            .maybe_sparse_index_host(maybe_env("DOCSRS_SPARSE_INDEX_HOST")?)
            .build())
    }
}
