use anyhow::Result;
use docs_rs_config::AppConfig;
use docs_rs_env_vars::maybe_env;
use docs_rs_types::Duration;
use std::path::PathBuf;

/// Configuration for [`crate::RustsecClient`].
#[derive(Debug, bon::Builder)]
#[builder(on(_, overwritable))]
pub struct Config {
    #[builder(default = rustsec::Repository::default_path())]
    pub advisory_db: PathBuf,

    #[builder(default = Duration::from_hours(1))]
    pub refresh_frequency: Duration,
}

impl AppConfig for Config {
    fn from_environment() -> Result<Self> {
        Ok(Self::builder()
            .maybe_advisory_db(maybe_env("DOCSRS_RUSTSEC_ADVISORY_DB")?)
            .maybe_refresh_frequency(maybe_env("DOCSRS_RUSTSEC_REFRESH_FREQUENCY")?)
            .build())
    }
}
