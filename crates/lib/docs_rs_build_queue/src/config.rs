use anyhow::Result;
use docs_rs_config::AppConfig;
use docs_rs_env_vars::maybe_env;
use std::time::Duration;

#[derive(Debug)]
pub struct Config {
    pub build_attempts: u16,
    pub deprioritize_workspace_size: u16,
    pub delay_between_build_attempts: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            build_attempts: 5,
            deprioritize_workspace_size: 20,
            delay_between_build_attempts: Duration::from_secs(60),
        }
    }
}

impl AppConfig for Config {
    fn from_environment() -> Result<Self> {
        let mut config = Self::default();

        if let Some(attempts) = maybe_env::<u16>("DOCSRS_BUILD_ATTEMPTS")? {
            config.build_attempts = attempts;
        }

        if let Some(delay) = maybe_env::<u64>("DOCSRS_DELAY_BETWEEN_BUILD_ATTEMPTS")? {
            config.delay_between_build_attempts = Duration::from_secs(delay);
        }

        if let Some(size) = maybe_env::<u16>("DOCSRS_DEPRIORITIZE_WORKSPACE_SIZE")? {
            config.deprioritize_workspace_size = size;
        }

        Ok(config)
    }
}
