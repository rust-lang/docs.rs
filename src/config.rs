use failure::{bail, format_err, Error, Fail, ResultExt};
use std::env::VarError;
use std::str::FromStr;

#[derive(Debug)]
pub struct Config {
    // Build params
    pub(crate) build_attempts: u16,

    // Database connection params
    pub(crate) database_url: String,
    pub(crate) max_pool_size: u32,
    pub(crate) min_pool_idle: u32,

    // Github authentication
    pub(crate) github_username: Option<String>,
    pub(crate) github_accesstoken: Option<String>,

    // Max size of the files served by the docs.rs frontend
    pub(crate) max_file_size: usize,
    pub(crate) max_file_size_html: usize,
}

impl Config {
    pub fn from_env() -> Result<Self, Error> {
        Ok(Self {
            build_attempts: env("DOCSRS_BUILD_ATTEMPTS", 5)?,

            database_url: require_env("CRATESFYI_DATABASE_URL")?,
            max_pool_size: env("DOCSRS_MAX_POOL_SIZE", 90)?,
            min_pool_idle: env("DOCSRS_MIN_POOL_IDLE", 10)?,

            github_username: maybe_env("CRATESFYI_GITHUB_USERNAME")?,
            github_accesstoken: maybe_env("CRATESFYI_GITHUB_ACCESSTOKEN")?,

            max_file_size: env("DOCSRS_MAX_FILE_SIZE", 50 * 1024 * 1024)?,
            max_file_size_html: env("DOCSRS_MAX_FILE_SIZE_HTML", 5 * 1024 * 1024)?,
        })
    }

    pub fn github_auth(&self) -> Option<(&str, &str)> {
        Some((
            self.github_username.as_deref()?,
            self.github_accesstoken.as_deref()?,
        ))
    }
}

fn env<T>(var: &str, default: T) -> Result<T, Error>
where
    T: FromStr,
    T::Err: Fail,
{
    Ok(maybe_env(var)?.unwrap_or(default))
}

fn require_env<T>(var: &str) -> Result<T, Error>
where
    T: FromStr,
    T::Err: Fail,
{
    maybe_env(var)?.ok_or_else(|| format_err!("configuration variable {} is missing", var))
}

fn maybe_env<T>(var: &str) -> Result<Option<T>, Error>
where
    T: FromStr,
    T::Err: Fail,
{
    match std::env::var(var) {
        Ok(content) => Ok(content
            .parse::<T>()
            .map(Some)
            .with_context(|_| format!("failed to parse configuration variable {}", var))?),
        Err(VarError::NotPresent) => {
            log::debug!("optional configuration variable {} is not set", var);
            Ok(None)
        }
        Err(VarError::NotUnicode(_)) => bail!("configuration variable {} is not UTF-8", var),
    }
}
