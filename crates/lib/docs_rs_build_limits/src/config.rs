use anyhow::Result;
use docs_rs_config::AppConfig;
use docs_rs_env_vars::maybe_env;

#[derive(Debug, Default)]
pub struct Config {
    pub(crate) build_default_memory_limit: Option<usize>,
}

impl AppConfig for Config {
    fn from_environment() -> Result<Self> {
        Ok(Self {
            build_default_memory_limit: maybe_env("DOCSRS_BUILD_DEFAULT_MEMORY_LIMIT")?,
        })
    }
}
