use crate::{Config as BuilderConfig, docbuilder::rustwide_builder::RustwideBuilder};
use anyhow::{Context as _, Result};
use docs_rs_build_queue::BuildQueue;
use docs_rs_context::Context;
use docs_rs_database::{Config as DatabaseConfig, testing::TestDatabase};
use docs_rs_fastly::Cdn;
use docs_rs_opentelemetry::testing::TestMetrics;
use docs_rs_storage::{Config as StorageConfig, Storage, StorageKind, testing::TestStorage};
use std::sync::Arc;
use tokio::runtime;

pub(crate) struct TestEnvironment {
    pub(crate) context: Context,
    pub(crate) config: Arc<BuilderConfig>,
    #[allow(dead_code)] // so we can allow asserting collected metrics later.
    pub(crate) metrics: TestMetrics,
    #[allow(dead_code)] // we need to keep the storage so it can be cleaned up.
    pub(crate) storage: TestStorage,
    pub(crate) db: TestDatabase,
    pub(crate) runtime: runtime::Runtime,
}

impl TestEnvironment {
    pub(crate) fn new_with_runtime() -> Result<Self> {
        Self::with_config_and_runtime(BuilderConfig::test_config()?)
    }

    pub(crate) fn with_config_and_runtime(config: BuilderConfig) -> Result<Self> {
        crate::logging::init();
        docs_rs_logging::testing::init();

        let runtime = runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("failed to initialize runtime")?;

        let metrics = TestMetrics::new();

        let db_config = DatabaseConfig::test_config()?;
        let db = runtime.block_on(TestDatabase::new(&db_config, metrics.provider()))?;

        let storage_config = Arc::new(StorageConfig::test_config(StorageKind::Memory)?);
        let test_storage = runtime.block_on(TestStorage::from_config(
            storage_config.clone(),
            metrics.provider(),
        ))?;

        Ok(Self {
            config: Arc::new(config),
            context: runtime.block_on(async {
                Context::builder()
                    .await?
                    .pool(db_config.into(), db.pool().clone())
                    .storage(storage_config.clone(), test_storage.storage())
                    .with_build_queue()
                    .await?
                    .maybe_cdn(
                        docs_rs_fastly::Config::from_environment()?.into(),
                        Some(Cdn::mock().into()),
                    )
                    .build()
            })?,
            db,
            storage: test_storage,
            metrics,
            runtime,
        })
    }

    pub(crate) fn runtime(&self) -> &runtime::Runtime {
        &self.runtime
    }

    pub(crate) fn storage(&self) -> Result<&Arc<Storage>> {
        self.context.blocking_storage()
    }

    pub(crate) fn cdn(&self) -> Result<&Arc<Cdn>> {
        self.context.cdn()
    }

    pub(crate) fn blocking_build_queue(&self) -> Result<&Arc<BuildQueue>> {
        self.context.blocking_build_queue()
    }

    pub(crate) fn build_builder(&self) -> Result<RustwideBuilder> {
        RustwideBuilder::init(self.config.clone(), &self.context)
    }
}
