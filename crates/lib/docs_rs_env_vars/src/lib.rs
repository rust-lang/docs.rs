use anyhow::{Context as _, Result, anyhow};
use std::{env::VarError, error::Error, str::FromStr};
use tracing::trace;

pub fn env<T>(var: &str, default: T) -> Result<T>
where
    T: FromStr,
    T::Err: Error + Send + Sync + 'static,
{
    Ok(maybe_env(var)?.unwrap_or(default))
}

pub fn require_env<T>(var: &str) -> Result<T>
where
    T: FromStr,
    <T as FromStr>::Err: Error + Send + Sync + 'static,
{
    maybe_env(var)?.with_context(|| anyhow!("configuration variable {} is missing", var))
}

pub fn maybe_env<T>(var: &str) -> Result<Option<T>>
where
    T: FromStr,
    T::Err: Error + Send + Sync + 'static,
{
    match std::env::var(var) {
        Ok(content) => Ok(content
            .parse::<T>()
            .map(Some)
            .with_context(|| format!("failed to parse configuration variable {var}"))?),
        Err(VarError::NotPresent) => {
            trace!("optional configuration variable {} is not set", var);
            Ok(None)
        }
        Err(VarError::NotUnicode(_)) => Err(anyhow!("configuration variable {} is not UTF-8", var)),
    }
}
