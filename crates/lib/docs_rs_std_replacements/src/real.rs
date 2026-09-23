use crate::{Config, ReplacementDetails, ReplacementMap, StdReplacementsProvider};
use anyhow::Result;
use async_trait::async_trait;
use docs_rs_types::KrateName;
use docs_rs_utils::APP_USER_AGENT;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use std::{fmt, sync::Arc, time::Duration};
use tokio::{
    sync::{OnceCell, RwLock},
    task::JoinHandle,
    time::{Instant, MissedTickBehavior},
};
use tracing::{debug, error, info, instrument};
use url::Url;

/// how often should our background task refetch the std replacements.
const REFRESH_INTERVAL: Duration = Duration::from_hours(1);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

type Cache = RwLock<Arc<ReplacementMap>>;

/// A cached client that refreshes independently of lookups.
/// Dropping the client aborts its background task, including any in-flight refresh.
pub struct StdReplacementsImpl {
    client: ClientWithMiddleware,
    url: Url,
    state: OnceCell<StdReplacementsInner>,
}

impl fmt::Debug for StdReplacementsImpl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StdReplacementsImpl")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

struct StdReplacementsInner {
    cache: Arc<Cache>,
    refresh_task: JoinHandle<()>,
}

impl StdReplacementsInner {
    async fn initialize(client: &ClientWithMiddleware, url: &Url) -> Result<Self> {
        let initial = Arc::new(fetch(client, url).await?);
        let cache = Arc::new(RwLock::new(initial));

        // The initial fetch already ran, so the first tick is one interval away.
        let mut interval =
            tokio::time::interval_at(Instant::now() + REFRESH_INTERVAL, REFRESH_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let refresh_task = tokio::spawn({
            let client = client.clone();
            let url = url.clone();
            let cache = cache.clone();
            async move {
                loop {
                    interval.tick().await;
                    if let Err(error) = refresh(&client, &url, &cache).await {
                        error!(?error, "failed to refresh standard-library replacements");
                    }
                }
            }
        });

        debug!("started standard-library replacement refresh task");
        Ok(Self {
            cache,
            refresh_task,
        })
    }
}

impl StdReplacementsImpl {
    /// Create a client without fetching data or starting a task.
    ///
    /// The first lookup loads the initial snapshot and starts hourly refreshes.
    /// Initial fetch failures fail that lookup and can be retried by another lookup.
    /// Later failures are logged and retain the last successful data.
    pub fn from_config(config: &Config) -> Result<Self> {
        let client = ClientBuilder::new(
            reqwest::Client::builder()
                .user_agent(APP_USER_AGENT)
                .timeout(REQUEST_TIMEOUT)
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
            state: OnceCell::new(),
        })
    }

    async fn state(&self) -> Result<&StdReplacementsInner> {
        self.state
            .get_or_try_init(|| StdReplacementsInner::initialize(&self.client, &self.url))
            .await
    }

    #[cfg(test)]
    async fn refresh(&self) -> Result<()> {
        let cache = self.state().await?.cache.clone();
        refresh(&self.client, &self.url, &cache).await?;

        Ok(())
    }
}

impl Drop for StdReplacementsImpl {
    fn drop(&mut self) {
        if let Some(state) = self.state.get() {
            state.refresh_task.abort();
            debug!("stopping standard-library replacement refresh task");
        }
    }
}

/// Fetch and publish a new snapshot.
async fn refresh(client: &ClientWithMiddleware, url: &Url, cache: &Cache) -> Result<()> {
    let replacements = fetch(client, url).await?;
    *cache.write().await = Arc::new(replacements);
    Ok(())
}

#[instrument(skip_all)]
async fn fetch(client: &ClientWithMiddleware, url: &Url) -> Result<ReplacementMap> {
    debug!("fetching standard-library replacements");
    let replacements: ReplacementMap = client
        .get(url.clone())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    info!(
        entries = replacements.len(),
        "fetched standard-library replacements"
    );
    Ok(replacements)
}

#[async_trait]
impl StdReplacementsProvider for StdReplacementsImpl {
    /// Initialize on the first lookup, then read the last successful snapshot.
    /// Concurrent first lookups share initialization. Later lookups do not wait
    /// for background refreshes.
    async fn get(&self, name: &KrateName) -> Result<Option<Arc<ReplacementDetails>>> {
        Ok(self.state().await?.cache.read().await.get(name).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::std_replacement;
    use docs_rs_types::testing::KRATE;
    use reqwest::StatusCode;
    use test_case::test_case;

    struct HttpTestFixture {
        server: mockito::ServerGuard,
        config: Config,
    }

    impl HttpTestFixture {
        async fn new() -> Result<Self> {
            let server = mockito::Server::new_async().await;
            let config = Config::builder()
                .url(server.url().parse()?)
                .max_retries(0)
                .build();
            Ok(Self { server, config })
        }

        async fn mock(&mut self, data: ReplacementMap) -> mockito::Mock {
            self.server
                .mock("GET", "/")
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(serde_json::to_vec(&data).unwrap())
                .create_async()
                .await
        }

        async fn mock_std_replacement(
            &mut self,
            krate: KrateName,
            details: ReplacementDetails,
        ) -> mockito::Mock {
            self.mock_std_replacements([(krate, details)]).await
        }

        async fn mock_std_replacements(
            &mut self,
            replacements: impl IntoIterator<Item = (KrateName, ReplacementDetails)>,
        ) -> mockito::Mock {
            self.mock(std_replacements(replacements)).await
        }

        async fn api(&self) -> Result<StdReplacementsImpl> {
            StdReplacementsImpl::from_config(&self.config)
        }
    }

    // Advance only the refresh timer; run HTTP I/O with real time so the runtime's
    // automatic time advancement cannot trigger extra refreshes while waiting for sockets.
    async fn tick() {
        tokio::time::pause();
        tokio::time::advance(REFRESH_INTERVAL).await;
        tokio::time::resume();
    }

    fn std_replacements(
        replacements: impl IntoIterator<Item = (KrateName, ReplacementDetails)>,
    ) -> ReplacementMap {
        ReplacementMap::from_iter(
            replacements
                .into_iter()
                .map(|(krate, replacement)| (krate, Arc::new(replacement))),
        )
    }

    #[tokio::test]
    async fn lookups_read_the_initial_snapshot_without_fetching() -> Result<()> {
        let mut env = HttpTestFixture::new().await?;
        let mock = env
            .mock_std_replacement(KRATE, std_replacement("old"))
            .await;
        let api = env.api().await?;
        let (first, second) = {
            let name = KRATE;
            tokio::try_join!(api.get(&name), api.get(&name))?
        };
        assert!(Arc::ptr_eq(&first.unwrap(), &second.unwrap()));
        assert!(api.get(&KrateName::from_static("missing")).await?.is_none());
        mock.assert_async().await;
        Ok(())
    }

    #[tokio::test]
    async fn refresh_replaces_the_snapshot() -> Result<()> {
        let mut env = HttpTestFixture::new().await?;
        let original = std_replacements([(KRATE, std_replacement("old"))]);
        let mock = env.mock(original.clone()).await;
        let api = env.api().await?;
        let old = api.get(&KRATE).await?.unwrap();
        mock.assert_async().await;
        mock.remove_async().await;

        for current in [
            original.clone(),
            std_replacements([(KRATE, std_replacement("new"))]),
            ReplacementMap::new(),
        ] {
            let mock = env.mock(current.clone()).await;
            api.refresh().await?;
            assert_eq!(api.get(&KRATE).await?, current.get(&KRATE).cloned());
            mock.assert_async().await;
            mock.remove_async().await;
        }
        assert_eq!(old.description(), "old");
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_first_lookups_share_initialization() -> Result<()> {
        let mut env = HttpTestFixture::new().await?;
        let mock = env
            .mock_std_replacement(KRATE, std_replacement("initial"))
            .await;
        // doesn't use `env.api()` since that already loads the initial data.
        let api = env.api().await?;
        let name = KRATE;
        let (first, second) = tokio::try_join!(api.get(&name), api.get(&name))?;
        assert!(Arc::ptr_eq(&first.unwrap(), &second.unwrap()));
        mock.assert_async().await;
        Ok(())
    }

    #[tokio::test]
    async fn background_task_refreshes_cached_data() -> Result<()> {
        let mut env = HttpTestFixture::new().await?;
        let initial = env.mock(ReplacementMap::new()).await;
        let api = env.api().await?;
        assert!(api.get(&KRATE).await?.is_none());
        initial.assert_async().await;
        initial.remove_async().await;

        let mock = env
            .mock_std_replacement(KRATE, std_replacement("updated"))
            .await;
        tick().await;

        // Once initialized, lookups only read the cache; the background task
        // must fetch and publish the updated value.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(replacement) = api.get(&KRATE).await? {
                    assert_eq!(replacement.description(), "updated");
                    break Ok::<_, anyhow::Error>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await??;
        mock.assert_async().await;
        Ok(())
    }

    #[tokio::test]
    async fn empty_initial_snapshot_is_cached() -> Result<()> {
        let mut env = HttpTestFixture::new().await?;
        let mock = env.mock(ReplacementMap::new()).await;
        let api = env.api().await?;
        for _ in 0..2 {
            assert!(api.get(&KRATE).await?.is_none());
        }
        mock.assert_async().await;
        Ok(())
    }

    #[test_case(StatusCode::NOT_FOUND, "not found"; "http error")]
    #[test_case(StatusCode::INTERNAL_SERVER_ERROR, "server error"; "server error")]
    #[test_case(StatusCode::OK, "invalid json"; "malformed json")]
    #[test_case(StatusCode::OK, r#"{"krate":{"description":"missing url"}}"#; "invalid details")]
    #[tokio::test]
    async fn initial_fetch_failure_returns_error(status: StatusCode, body: &str) -> Result<()> {
        let mut env = HttpTestFixture::new().await?;
        let mock = env
            .server
            .mock("GET", "/")
            .with_status(status.as_u16().into())
            .with_body(body)
            .create_async()
            .await;
        let api = env.api().await?;
        let error = api.get(&KRATE).await.unwrap_err();
        assert!(api.state.get().is_none());
        let error = error.downcast_ref::<reqwest::Error>().unwrap();
        if status.is_success() {
            assert!(error.is_decode());
        } else {
            assert_eq!(error.status(), Some(status));
        }
        mock.assert_async().await;
        Ok(())
    }

    #[tokio::test]
    async fn failed_refresh_preserves_cache_and_can_be_retried() -> Result<()> {
        let mut env = HttpTestFixture::new().await?;
        let initial = env
            .mock_std_replacement(KRATE, std_replacement("old"))
            .await;
        let api = env.api().await?;
        let old = api.get(&KRATE).await?.unwrap();
        initial.remove_async().await;
        let failure = env
            .server
            .mock("GET", "/")
            .with_status(500)
            .create_async()
            .await;
        assert!(api.refresh().await.is_err());
        failure.assert_async().await;
        assert!(Arc::ptr_eq(&api.get(&KRATE).await?.unwrap(), &old));
        failure.remove_async().await;
        let recovered = env
            .mock_std_replacement(KRATE, std_replacement("recovered"))
            .await;
        api.refresh().await?;
        assert_eq!(api.get(&KRATE).await?.unwrap().description(), "recovered");
        recovered.assert_async().await;
        Ok(())
    }

    #[test_case(false; "sleeping")]
    #[test_case(true; "refresh in flight")]
    #[tokio::test]
    async fn drop_stops_background_task(in_refresh: bool) -> Result<()> {
        let mut env = HttpTestFixture::new().await?;
        let initial = env.mock(ReplacementMap::new()).await;
        let api = env.api().await?;
        api.get(&KRATE).await?; // triggers the initial data load
        initial.remove_async().await;
        let cache = api.state().await?.cache.clone();
        // Keep a refresh from completing publication while testing cancellation.
        let guard = cache.read().await;
        if in_refresh {
            let mock = env.mock(ReplacementMap::new()).await;
            tick().await;
            tokio::time::timeout(Duration::from_secs(5), async {
                while !mock.matched_async().await {
                    tokio::task::yield_now().await;
                }
            })
            .await?;
        }
        let weak_cache = Arc::downgrade(&cache);
        let task = api.state().await?.refresh_task.abort_handle();
        drop(api);
        tokio::time::timeout(Duration::from_secs(5), async {
            while !task.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        drop(guard);
        drop(cache);
        assert!(weak_cache.upgrade().is_none());
        Ok(())
    }

    #[tokio::test]
    async fn construction_does_not_fetch_or_start_task() -> Result<()> {
        let mut env = HttpTestFixture::new().await?;
        let mock = env.server.mock("GET", "/").expect(0).create_async().await;
        let api = env.api().await?;
        tick().await;
        assert!(api.state.get().is_none());
        drop(api);
        mock.assert_async().await;
        Ok(())
    }

    #[tokio::test]
    async fn failed_initial_load_can_be_retried() -> Result<()> {
        let mut env = HttpTestFixture::new().await?;
        let failure = env
            .server
            .mock("GET", "/")
            .with_status(500)
            .create_async()
            .await;
        let api = env.api().await?;
        assert!(api.get(&KRATE).await.is_err());
        assert!(api.state.get().is_none());
        failure.assert_async().await;
        failure.remove_async().await;
        let mock = env
            .mock_std_replacement(KRATE, std_replacement("recovered"))
            .await;
        assert_eq!(api.get(&KRATE).await?.unwrap().description(), "recovered");
        assert!(api.state.get().is_some());
        mock.assert_async().await;
        Ok(())
    }
}
