use docs_rs_env_vars::{env, maybe_env, require_env};
use std::{path::PathBuf, time::Duration};
use url::Url;

#[derive(Debug)]
pub struct Config {}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {}
}
