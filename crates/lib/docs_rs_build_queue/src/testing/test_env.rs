use crate::{AsyncBuildQueue, BuildQueue, Config};
use anyhow::Result;
use docs_rs_config::AppConfig as _;
use docs_rs_database::testing::TestDatabase;
use docs_rs_opentelemetry::testing::TestMetrics;
use docs_rs_storage::testing::TestStorage;
use docs_rs_test_fakes::FakeRelease;
use std::sync::Arc;
use tokio::runtime;

pub(crate) struct TestEnv {
    metrics: TestMetrics,
    pub(crate) db: TestDatabase,
    pub(crate) storage: TestStorage,
}

impl TestEnv {
    pub(crate) async fn fake_release(&self) -> FakeRelease<'_> {
        FakeRelease::new(self.db.pool().clone(), self.storage.storage().clone())
    }

    pub(crate) async fn new() -> Result<TestEnv> {
        let metrics = TestMetrics::new();
        let db = TestDatabase::new(
            &docs_rs_database::Config::test_config()?,
            metrics.provider(),
        )
        .await?;

        let storage = TestStorage::from_config(
            docs_rs_storage::Config::test_config()?.into(),
            metrics.provider(),
        )
        .await?;

        Ok(TestEnv {
            metrics,
            db,
            storage,
        })
    }

    pub(crate) fn queue(&self) -> AsyncBuildQueue {
        self.queue_with_config(Config::default())
    }

    pub(crate) fn queue_with_config(&self, config: Config) -> AsyncBuildQueue {
        AsyncBuildQueue::new(
            self.db.pool().clone(),
            Arc::new(config),
            self.metrics.provider(),
        )
    }
}

pub(crate) struct BlockingTestEnv {
    inner: TestEnv,
    #[allow(dead_code)] // we need to keep the runtime alive while using the inner environment
    runtime: runtime::Runtime,
}

impl BlockingTestEnv {
    pub(crate) fn new() -> Result<BlockingTestEnv> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        Ok(BlockingTestEnv {
            inner: runtime.block_on(TestEnv::new())?,
            runtime,
        })
    }

    pub(crate) fn queue(&self) -> BuildQueue {
        self.queue_with_config(Config::default())
    }

    pub(crate) fn queue_with_config(&self, config: Config) -> BuildQueue {
        let async_queue = self.inner.queue_with_config(config);

        BuildQueue {
            runtime: self.runtime.handle().clone().into(),
            inner: async_queue.into(),
        }
    }

    pub(crate) fn queued_builds(&self) -> Result<u64> {
        let collected_metrics = self.inner.metrics.collected_metrics();

        Ok(collected_metrics
            .get_metric("build_queue", "docsrs.build_queue.queued_builds")?
            .get_u64_counter()
            .value())
    }

    pub(crate) fn failed_count(&self) -> u64 {
        let collected_metrics = self.inner.metrics.collected_metrics();

        if let Ok(metric) =
            collected_metrics.get_metric("build_queue", "docsrs.build_queue.failed_crates_count")
        {
            metric.get_u64_counter().value()
        } else {
            0
        }
    }

    pub(crate) fn block_on_async_with_conn<R>(
        &self,
        f: impl AsyncFnOnce(&mut sqlx::PgConnection) -> Result<R>,
    ) -> Result<R> {
        self.runtime.block_on(async {
            let mut conn = self.inner.db.async_conn().await?;
            f(&mut conn).await
        })
    }
}
