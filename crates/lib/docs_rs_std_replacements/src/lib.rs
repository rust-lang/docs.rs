//! Cached standard-library alternatives to third-party crates.
mod config;
mod models;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
#[cfg(test)]
mod tests;

pub use config::Config;
use docs_rs_types::KrateName;
use docs_rs_utils::APP_USER_AGENT;
pub use models::{ReplacementDetails, ReplacementMap};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use url::Url;

const CACHE_TTL: Duration = Duration::from_hours(1);

#[derive(Debug)]
struct CachedStdReplacements {
    data: ReplacementMap,
    fetched_at: Instant,
}

/// A shared client which lazily fetches and caches the complete replacement list.
#[derive(Debug)]
pub struct StdReplacements {
    client: ClientWithMiddleware,
    url: Url,
    cache: Mutex<Option<CachedStdReplacements>>,
}

impl StdReplacements {
    /// Create a client without fetching data until the first lookup.
    pub fn from_config(config: &Config) -> anyhow::Result<Self> {
        let client = ClientBuilder::new(
            reqwest::Client::builder()
                .user_agent(APP_USER_AGENT)
                .gzip(true)
                .build()?,
        )
        .with(RetryTransientMiddleware::new_with_policy(
            ExponentialBackoff::builder().build_with_max_retries(config.max_retries),
        ))
        .build();
        Ok(Self {
            client,
            url: config.url.clone(),
            cache: Mutex::new(None),
        })
    }

    /// Return a crate's standard-library alternative, if one is known.
    ///
    /// The complete dataset is fetched lazily and cached for one hour. Concurrent
    /// lookups share a fetch. Failed refreshes return an error and retain the old
    /// cache; returned entries remain valid across subsequent refreshes.
    ///
    /// See https://github.com/rust-lang/std-replacement-data
    pub async fn get(&self, name: &KrateName) -> anyhow::Result<Option<Arc<ReplacementDetails>>> {
        let mut cached_replacements = self.cache.lock().await;

        if cached_replacements
            .as_ref()
            .is_none_or(|cached_replacements| cached_replacements.fetched_at.elapsed() >= CACHE_TTL)
        {
            let new_replacements: ReplacementMap = self
                .client
                .get(self.url.clone())
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;

            *cached_replacements = Some(CachedStdReplacements {
                data: new_replacements,
                fetched_at: Instant::now(),
            });
        }

        Ok(cached_replacements
            .as_ref()
            .expect("always exists here because we fetch above")
            .data
            .get(name)
            .cloned())
    }
}
