use crate::Config as WatcherConfig;
use anyhow::Result;
use docs_rs_build_queue::AsyncBuildQueue;
use docs_rs_context::Context;
use docs_rs_database::{AsyncPoolClient, Config as DatabaseConfig, testing::TestDatabase};
use docs_rs_fastly::Cdn;
use docs_rs_opentelemetry::testing::TestMetrics;
use docs_rs_storage::{AsyncStorage, Config as StorageConfig, StorageKind, testing::TestStorage};
use docs_rs_test_fakes::FakeRelease;
use std::sync::Arc;

pub(crate) struct TestEnvironment {
    pub(crate) context: Context,
    pub(crate) config: Arc<WatcherConfig>,
    #[allow(dead_code)] // so we can allow asserting collected metrics later.
    pub(crate) metrics: TestMetrics,
    #[allow(dead_code)] // we need to keep the storage so it can be cleaned up.
    pub(crate) storage: TestStorage,
    #[allow(dead_code)] // we need to keep the storage so it can be cleaned up.
    pub(crate) db: TestDatabase,
}

impl TestEnvironment {
    pub(crate) async fn new() -> Result<Self> {
        Self::with_config(WatcherConfig::test_config()?).await
    }

    pub(crate) async fn with_config(config: WatcherConfig) -> Result<Self> {
        docs_rs_logging::testing::init();

        let metrics = TestMetrics::new();

        let db_config = DatabaseConfig::test_config()?;
        let db = TestDatabase::new(&db_config, metrics.provider()).await?;

        let storage_config = Arc::new(StorageConfig::test_config(StorageKind::Memory)?);
        let test_storage =
            TestStorage::from_config(storage_config.clone(), metrics.provider()).await?;

        Ok(Self {
            config: Arc::new(config),
            context: Context::builder()
                .await?
                .pool(db_config.into(), db.pool().clone())
                .storage(storage_config.clone(), test_storage.storage())
                .with_build_queue()
                .await?
                .maybe_cdn(
                    docs_rs_fastly::Config::from_environment()?.into(),
                    Some(Cdn::mock().into()),
                )
                .with_repository_stats()
                .await?
                .build()?,
            db,
            storage: test_storage,
            metrics,
        })
    }

    pub(crate) fn config(&self) -> &WatcherConfig {
        &self.config
    }

    pub(crate) fn build_queue(&self) -> Result<&Arc<AsyncBuildQueue>> {
        self.context.build_queue()
    }

    pub(crate) async fn async_conn(&self) -> Result<AsyncPoolClient> {
        self.context.pool()?.get_async().await.map_err(Into::into)
    }

    pub(crate) fn storage(&self) -> Result<&Arc<AsyncStorage>> {
        self.context.storage()
    }

    pub async fn fake_release(&self) -> FakeRelease<'_> {
        FakeRelease::new(
            self.context.pool().unwrap().clone(),
            self.context.storage().unwrap().clone(),
        )
    }
}
