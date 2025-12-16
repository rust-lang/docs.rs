use docs_rs_env_vars::{env, require_env};

#[derive(Debug)]
pub struct Config {
    pub database_url: String,
    pub max_pool_size: u32,
    pub min_pool_idle: u32,
}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {
        Ok(Self {
            database_url: require_env("DOCSRS_DATABASE_URL")?,
            max_pool_size: env("DOCSRS_MAX_POOL_SIZE", 90u32)?,
            min_pool_idle: env("DOCSRS_MIN_POOL_IDLE", 10u32)?,
        })
    }
}
