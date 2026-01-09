use anyhow::Result;
use docs_rs_config::AppConfig;
use docs_rs_env_vars::{env, require_env};

#[derive(Debug)]
pub struct Config {
    pub database_url: String,
    pub max_pool_size: u32,
    pub min_pool_idle: u32,
}

impl AppConfig for Config {
    fn from_environment() -> Result<Self> {
        Ok(Self {
            database_url: require_env("DOCSRS_DATABASE_URL")?,
            max_pool_size: env("DOCSRS_MAX_POOL_SIZE", 90u32)?,
            min_pool_idle: env("DOCSRS_MIN_POOL_IDLE", 10u32)?,
        })
    }

    #[cfg(any(feature = "testing", test))]
    fn test_config() -> Result<Self> {
        let mut config = Self::from_environment()?;

        // Use less connections for each test compared to production.
        config.max_pool_size = 8;
        config.min_pool_idle = 2;

        Ok(config)
    }
}
