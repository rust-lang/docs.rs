use docs_rs_env_vars::{env, maybe_env};

#[derive(Debug)]
pub struct Config {
    // Github authentication
    pub(crate) github_accesstoken: Option<String>,
    pub(crate) github_updater_min_rate_limit: u32,

    // GitLab authentication
    pub(crate) gitlab_accesstoken: Option<String>,
}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {
        Ok(Self {
            github_accesstoken: maybe_env("DOCSRS_GITHUB_ACCESSTOKEN")?,
            github_updater_min_rate_limit: env("DOCSRS_GITHUB_UPDATER_MIN_RATE_LIMIT", 2500u32)?,
            gitlab_accesstoken: maybe_env("DOCSRS_GITLAB_ACCESSTOKEN")?,
        })
    }
}
