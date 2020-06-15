use failure::{bail, Error, Fail, ResultExt};
use std::env::VarError;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Debug)]
pub struct Config {
    pub(crate) max_file_size: usize,
    pub(crate) max_file_size_html: usize,
}

impl Config {
    pub fn from_env() -> Result<Self, Error> {
        Ok(Self {
            max_file_size: env("DOCSRS_MAX_FILE_SIZE", 50 * 1024 * 1024)?,
            max_file_size_html: env("DOCSRS_MAX_FILE_SIZE_HTML", 5 * 1024 * 1024)?,
        })
    }
}

impl iron::typemap::Key for Config {
    type Value = Arc<Config>;
}

fn env<T>(var: &str, default: T) -> Result<T, Error>
where
    T: FromStr,
    T::Err: Fail,
{
    match std::env::var(var) {
        Ok(content) => Ok(content
            .parse::<T>()
            .with_context(|_| format!("failed to parse configuration variable {}", var))?),
        Err(VarError::NotPresent) => Ok(default),
        Err(VarError::NotUnicode(_)) => bail!("configuration variable {} is not UTF-8", var),
    }
}
