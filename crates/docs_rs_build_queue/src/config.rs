use docs_rs_env_vars::{env, maybe_env};

#[derive(Debug)]
pub struct Config {
    pub(crate) build_attempts: u16,
    // automatic rebuild configuration
    pub(crate) max_queued_rebuilds: Option<u16>,
}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {
        Ok(Self {
            build_attempts: env("DOCSRS_BUILD_ATTEMPTS", 5u16)?,
            max_queued_rebuilds: maybe_env("DOCSRS_MAX_QUEUED_REBUILDS")?,
        })
    }
}
