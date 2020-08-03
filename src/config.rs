use failure::{bail, format_err, Error, Fail, ResultExt};
use std::env::VarError;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Debug)]
pub struct Config {
    // Build params
    pub(crate) build_attempts: u16,

    pub prefix: PathBuf,
    pub registry_index_path: PathBuf,

    // Database connection params
    pub(crate) database_url: String,
    pub(crate) max_pool_size: u32,
    pub(crate) min_pool_idle: u32,

    // S3 params
    pub(crate) s3_bucket: String,
    #[cfg(test)]
    pub(crate) s3_bucket_is_temporary: bool,

    // Github authentication
    pub(crate) github_username: Option<String>,
    pub(crate) github_accesstoken: Option<String>,

    // Max size of the files served by the docs.rs frontend
    pub(crate) max_file_size: usize,
    pub(crate) max_file_size_html: usize,
    // The most memory that can be used to parse an HTML file
    pub(crate) max_parse_memory: usize,
}

impl Config {
    pub fn from_env() -> Result<Self, Error> {
        let prefix: PathBuf = require_env("CRATESFYI_PREFIX")?;

        Ok(Self {
            build_attempts: env("DOCSRS_BUILD_ATTEMPTS", 5)?,

            prefix: prefix.clone(),
            registry_index_path: env("REGISTRY_INDEX_PATH", prefix.join("crates.io-index"))?,

            database_url: require_env("CRATESFYI_DATABASE_URL")?,
            max_pool_size: env("DOCSRS_MAX_POOL_SIZE", 90)?,
            min_pool_idle: env("DOCSRS_MIN_POOL_IDLE", 10)?,

            s3_bucket: env("DOCSRS_S3_BUCKET", "rust-docs-rs".to_string())?,
            // DO NOT CONFIGURE THIS THROUGH AN ENVIRONMENT VARIABLE!
            // Accidentally turning this on outside of the test suite might cause data loss in the
            // production environment.
            #[cfg(test)]
            s3_bucket_is_temporary: false,

            github_username: maybe_env("CRATESFYI_GITHUB_USERNAME")?,
            github_accesstoken: maybe_env("CRATESFYI_GITHUB_ACCESSTOKEN")?,

            max_file_size: env("DOCSRS_MAX_FILE_SIZE", 50 * 1024 * 1024)?,
            max_file_size_html: env("DOCSRS_MAX_FILE_SIZE_HTML", 50 * 1024 * 1024)?,
            // LOL HTML only uses as much memory as the size of the start tag!
            // https://github.com/rust-lang/docs.rs/pull/930#issuecomment-667729380
            max_parse_memory: env("DOCSRS_MAX_PARSE_MEMORY", 5 * 1024 * 1024)?,
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
