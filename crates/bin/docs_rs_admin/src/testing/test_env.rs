use anyhow::Result;
use bon::bon;
use docs_rs_build_queue::AsyncBuildQueue;
use docs_rs_context::Context;
use docs_rs_database::{AsyncPoolClient, Config as DatabaseConfig, testing::TestDatabase};
use docs_rs_opentelemetry::testing::TestMetrics;
use docs_rs_storage::{Config as StorageConfig, StorageKind, testing::TestStorage};
use docs_rs_test_fakes::FakeRelease;
use std::sync::Arc;

pub(crate) struct TestEnvironment {
    pub(crate) context: Arc<Context>,
    // so we can allow asserting collected metrics later.
    #[allow(dead_code)] // we need to keep the metrics so they can be collected & cleaned
    pub(crate) metrics: TestMetrics,
    #[allow(dead_code)] // we need to keep the storage so it can be cleaned up.
    pub(crate) storage: TestStorage,
    #[allow(dead_code)] // we need to keep the storage so it can be cleaned up.
    pub(crate) db: TestDatabase,
}

#[bon]
impl TestEnvironment {
    pub(crate) async fn new() -> Result<Self> {
        // NOTE: compiler crashes when I change the return to
        // `Self::builder().build().await
        #[allow(clippy::needless_question_mark)]
        Ok(Self::builder().build().await?)
    }

    #[builder(finish_fn = build)]
    pub(crate) async fn builder() -> Result<Self> {
        docs_rs_logging::testing::init();

        let metrics = TestMetrics::new();

        let db_config = DatabaseConfig::test_config()?;
        let db = TestDatabase::new(&db_config, metrics.provider()).await?;

        let storage_config = Arc::new(StorageConfig::test_config(StorageKind::Memory)?);

        let test_storage =
            TestStorage::from_config(storage_config.clone(), metrics.provider()).await?;

        Ok(Self {
            context: Context::builder()
                .with_runtime()
                .await?
                .meter_provider(metrics.provider().clone())
                .pool(db_config.into(), db.pool().clone())
                .storage(storage_config.clone(), test_storage.storage())
                .with_build_queue()?
                .build()?
                .into(),
            db,
            storage: test_storage,
            metrics,
        })
    }

    pub(crate) fn build_queue(&self) -> Result<&Arc<AsyncBuildQueue>> {
        self.context.build_queue()
    }

    pub(crate) async fn async_conn(&self) -> Result<AsyncPoolClient> {
        self.context.pool()?.get_async().await.map_err(Into::into)
    }

    pub async fn fake_release(&self) -> FakeRelease<'_> {
        FakeRelease::new(
            self.context.pool().unwrap().clone(),
            self.context.storage().unwrap().clone(),
        )
    }
}
