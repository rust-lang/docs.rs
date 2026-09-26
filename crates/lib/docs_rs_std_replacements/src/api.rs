use crate::{Config, ReplacementDetails, ReplacementMap};
use anyhow::Result;
use arc_swap::ArcSwapOption;
use bon::bon;
use docs_rs_types::KrateName;
use docs_rs_utils::APP_USER_AGENT;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use std::{sync::Arc, time::Duration};
use tokio::task::JoinHandle;
use tracing::{error, info, instrument};
use url::Url;

#[derive(Debug)]
struct Inner {
    client: ClientWithMiddleware,
    url: Url,
    database: Arc<ArcSwapOption<ReplacementMap>>,
}

impl Inner {
    #[instrument(skip_all)]
    async fn refresh(&self) -> Result<()> {
        info!(url = %self.url, "refreshing standard-library replacements");

        let data = self
            .client
            .get(self.url.clone())
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        let loaded: ReplacementMap = serde_json::from_slice(&data)?;
        info!(entries = loaded.len(), "...done");

        self.database.store(Some(Arc::new(loaded)));
        Ok(())
    }
}

/// A replacement dataset refreshed in the background.
#[derive(Debug)]
pub struct StdReplacements {
    inner: Arc<Inner>,
    background_task: Option<JoinHandle<()>>,
}

#[bon]
impl StdReplacements {
    /// Refresh in the background at the configured frequency.
    pub async fn from_config(config: &Config) -> Result<Self> {
        Self::builder(config)
            .start_background_refresh(true)
            .initial_data_load(false)
            .build()
            .await
    }

    #[builder(on(_, into))]
    pub(crate) async fn new(
        #[builder(start_fn)] config: &Config,
        #[builder(default = true)] start_background_refresh: bool,
        #[builder(default = false)] initial_data_load: bool,
    ) -> Result<Self> {
        let client = ClientBuilder::new(
            reqwest::Client::builder()
                .user_agent(APP_USER_AGENT)
                .gzip(true)
                .timeout(Duration::from_secs(30))
                .build()?,
        )
        .with(RetryTransientMiddleware::new_with_policy(
            ExponentialBackoff::builder().build_with_max_retries(config.max_retries),
        ))
        .build();

        let inner = Arc::new(Inner {
            client,
            url: config.url.clone(),
            database: Arc::new(ArcSwapOption::empty()),
        });

        if initial_data_load {
            inner.refresh().await?;
        }

        let background_task = start_background_refresh.then(|| {
            tokio::spawn({
                let inner = inner.clone();
                let refresh_frequency: Duration = config.refresh_frequency.into();
                async move {
                    if initial_data_load {
                        // Fresh data was loaded for the test, so wait until the next cycle.
                        tokio::time::sleep(refresh_frequency).await;
                    }
                    loop {
                        if let Err(err) = inner.refresh().await {
                            error!(?err, url = %inner.url, "failed to load standard-library replacements");
                        }

                        tokio::time::sleep(refresh_frequency).await;
                    }
                }
            })
        });

        Ok(Self {
            inner,
            background_task,
        })
    }

    /// Fetch the current replacement dataset and publish it as a new snapshot.
    #[cfg(test)]
    async fn refresh(&self) -> Result<()> {
        self.inner.refresh().await
    }

    /// Return a replacement from the most recent snapshot.
    pub fn get(&self, name: &KrateName) -> Option<Arc<ReplacementDetails>> {
        self.database()
            .and_then(|database| database.get(name).cloned())
    }

    /// Return a stable snapshot, or None if the first load has not succeeded yet.
    pub fn database(&self) -> Option<Arc<ReplacementMap>> {
        self.inner.database.load_full()
    }
}

impl Drop for StdReplacements {
    fn drop(&mut self) {
        if let Some(background_task) = &self.background_task {
            background_task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{StdReplacementMockServer, std_replacement};
    use docs_rs_types::KrateName;
    use http::StatusCode;
    use test_case::test_case;

    const FAKE_PACKAGE: KrateName = KrateName::from_static("fake-package");

    async fn loaded_client(server: &StdReplacementMockServer) -> Result<StdReplacements> {
        StdReplacements::builder(&server.config().build())
            .start_background_refresh(false)
            .initial_data_load(true)
            .build()
            .await
    }

    #[tokio::test]
    async fn get_reads_loaded_snapshot() -> Result<()> {
        let server = StdReplacementMockServer::new().await;
        let server = server
            .mock()
            .replacement(FAKE_PACKAGE, std_replacement("Use std"))
            .start()
            .await;
        let api = loaded_client(&server).await?;
        let replacement = api.get(&FAKE_PACKAGE);
        assert_eq!(replacement.unwrap().description(), "Use std");
        server.assert_async().await;
        Ok(())
    }

    #[test_case(500, "failed"; "HTTP error")]
    #[test_case(404, "missing"; "missing dataset")]
    #[test_case(200, "invalid JSON"; "invalid dataset")]
    #[tokio::test]
    async fn refresh_retains_snapshot_on_failure_then_replaces_it(
        status: usize,
        body: &str,
    ) -> Result<()> {
        let mut server = StdReplacementMockServer::new()
            .await
            .mock()
            .replacement(FAKE_PACKAGE, std_replacement("Use std"))
            .start()
            .await;
        let api = loaded_client(&server).await?;
        let old = api.database().expect("loaded database");
        server.assert_and_remove_mock().await;
        server = server
            .mock()
            .status_code(StatusCode::from_u16(status as u16)?)
            .raw_body(body.to_owned())
            .start()
            .await;
        assert!(api.refresh().await.is_err());
        assert!(Arc::ptr_eq(&old, &api.database().unwrap()));
        server.assert_and_remove_mock().await;
        server = server.mock().start().await;
        api.refresh().await?;
        assert!(api.database().expect("updated database").is_empty());
        assert_eq!(old.len(), 1);
        server.assert_async().await;
        Ok(())
    }
}
