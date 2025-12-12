use docs_rs_env_vars::env;
use std::time::Duration;

#[derive(Debug)]
pub struct Config {
    pub build_attempts: u16,
    pub delay_between_build_attempts: Duration,
}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {
        Ok(Self {
            build_attempts: env("DOCSRS_BUILD_ATTEMPTS", 5u16)?,
            delay_between_build_attempts: Duration::from_secs(env::<u64>(
                "DOCSRS_DELAY_BETWEEN_BUILD_ATTEMPTS",
                60,
            )?),
        })
    }
}
