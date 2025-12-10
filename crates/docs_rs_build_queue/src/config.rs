use docs_rs_env_vars::env;

#[derive(Debug)]
pub struct Config {
    pub build_attempts: u16,
}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {
        Ok(Self {
            build_attempts: env("DOCSRS_BUILD_ATTEMPTS", 5u16)?,
        })
    }
}
