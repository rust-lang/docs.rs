use docs_rs_env_vars::maybe_env;

#[derive(Debug)]
pub struct Config {
    pub(crate) build_default_memory_limit: Option<usize>,
}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {
        Ok(Self {
            build_default_memory_limit: maybe_env("DOCSRS_BUILD_DEFAULT_MEMORY_LIMIT")?,
        })
    }
}
