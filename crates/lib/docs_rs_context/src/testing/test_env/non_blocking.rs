use crate::Context;
use anyhow::Result;
use bon::bon;
use docs_rs_config::AppConfig;
use docs_rs_database::{AsyncPoolClient, Config as DatabaseConfig, testing::TestDatabase};
use docs_rs_fastly::Cdn;
use docs_rs_opentelemetry::testing::{CollectedMetrics, TestMetrics};
use docs_rs_registry_api::RegistryApi;
use docs_rs_storage::{Config as StorageConfig, testing::TestStorage};
use docs_rs_test_fakes::FakeRelease;
use std::{ops::Deref, sync::Arc};

pub struct TestEnvironment<C> {
    context: Arc<Context>,
    config: Arc<C>,
    // so we can allow asserting collected metrics later.
    metrics: TestMetrics,
    #[allow(dead_code)] // we need to keep the storage so it can be cleaned up.
    storage: TestStorage,
    #[allow(dead_code)] // we need to keep the storage so it can be cleaned up.
    db: TestDatabase,
}

impl<C: AppConfig> Deref for TestEnvironment<C> {
    type Target = Context;

    fn deref(&self) -> &Self::Target {
        &self.context
    }
}

#[bon]
impl<C: AppConfig> TestEnvironment<C> {
    pub async fn new() -> Result<Self> {
        // NOTE: compiler crashes when I change the return to
        // `Self::builder().build().await
        #[allow(clippy::needless_question_mark)]
        Ok(Self::builder().build().await?)
    }

    #[builder(finish_fn = build)]
    pub async fn builder(
        config: Option<C>,
        registry_api_config: Option<docs_rs_registry_api::Config>,
        storage_config: Option<StorageConfig>,
    ) -> Result<Self> {
        docs_rs_logging::testing::init();

        let app_config = Arc::new(if let Some(web_config) = config {
            web_config
        } else {
            C::test_config()?
        });

        let registry_api_config =
            Arc::new(if let Some(registry_api_config) = registry_api_config {
                registry_api_config
            } else {
                docs_rs_registry_api::Config::from_environment()?
            });

        let registry_api = RegistryApi::from_config(&registry_api_config)?;

        let metrics = TestMetrics::new();

        let db_config = DatabaseConfig::test_config()?;
        let db = TestDatabase::new(&db_config, metrics.provider()).await?;

        let storage_config = Arc::new(if let Some(storage_config) = storage_config {
            storage_config
        } else {
            StorageConfig::test_config()?
        });

        let test_storage =
            TestStorage::from_config(storage_config.clone(), metrics.provider()).await?;

        Ok(Self {
            config: app_config,
            context: Context::builder()
                .with_runtime()
                .await?
                .meter_provider(metrics.provider().clone())
                .pool(db_config.into(), db.pool().clone())
                .storage(storage_config.clone(), test_storage.storage())
                .with_build_queue()?
                .registry_api(registry_api_config, registry_api.into())
                .with_repository_stats()?
                .maybe_cdn(
                    Arc::new(docs_rs_fastly::Config::test_config()?),
                    Some(Cdn::mock().into()),
                )
                .with_build_limits()?
                .build()?
                .into(),
            db,
            storage: test_storage,
            metrics,
        })
    }

    pub fn config(&self) -> &Arc<C> {
        &self.config
    }

    pub fn context(&self) -> &Arc<Context> {
        &self.context
    }

    pub fn cdn(&self) -> &Arc<Cdn> {
        self.context
            .cdn()
            .expect("we always have a CDN in test environments")
    }

    pub async fn async_conn(&self) -> Result<AsyncPoolClient> {
        self.context.pool()?.get_async().await.map_err(Into::into)
    }

    pub async fn fake_release(&self) -> FakeRelease<'_> {
        FakeRelease::new(
            self.context.pool().unwrap().clone(),
            self.context.storage().unwrap().clone(),
        )
    }

    pub fn collected_metrics(&self) -> CollectedMetrics {
        self.metrics.collected_metrics()
    }
}
