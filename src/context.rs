use crate::{Config, docbuilder::BuilderMetrics};
use anyhow::Result;
use docs_rs_build_queue::{AsyncBuildQueue, BuildQueue};
use docs_rs_database::Pool;
use docs_rs_fastly::Cdn;
use docs_rs_opentelemetry::{AnyMeterProvider, get_meter_provider};
use docs_rs_registry_api::RegistryApi;
use docs_rs_repository_stats::RepositoryStatsUpdater;
use docs_rs_storage::{AsyncStorage, Storage};
use std::sync::Arc;
use tokio::runtime;

pub struct Context {
    pub config: Arc<Config>,
    pub async_build_queue: Arc<AsyncBuildQueue>,
    pub builder_metrics: Arc<BuilderMetrics>, // temporary place until the refactor is finished
    pub build_queue: Arc<BuildQueue>,
    pub storage: Arc<Storage>,
    pub async_storage: Arc<AsyncStorage>,
    pub cdn: Option<Arc<Cdn>>,
    pub pool: Pool,
    pub registry_api: Arc<RegistryApi>,
    pub repository_stats_updater: Arc<RepositoryStatsUpdater>,
    pub runtime: runtime::Handle,
    pub meter_provider: AnyMeterProvider,
}

impl Context {
    /// Create a new context environment from the given configuration.
    pub async fn from_config(config: Config) -> Result<Self> {
        let meter_provider = get_meter_provider(&config.opentelemetry)?;
        let pool = Pool::new(&config.database, &meter_provider).await?;
        let cdn = config
            .fastly
            .is_valid()
            .then(|| Cdn::from_config(&config.fastly, &meter_provider))
            .transpose()?;

        let async_storage =
            Arc::new(AsyncStorage::new(config.storage.clone(), &meter_provider).await?);

        Self::from_parts(config, meter_provider, pool, async_storage, cdn).await
    }

    /// Create a new context environment from the given configuration, for running tests.
    #[cfg(test)]
    pub async fn from_test_config(
        config: Config,
        meter_provider: AnyMeterProvider,
        pool: Pool,
        async_storage: Arc<AsyncStorage>,
    ) -> Result<Self> {
        Self::from_parts(
            config,
            meter_provider,
            pool,
            async_storage,
            Some(Cdn::mock()),
        )
        .await
    }

    /// private function for context environment generation, allows passing in a
    /// preconfigured instance metrics & pool from the database.
    /// Mostly so we can support test environments with their db
    async fn from_parts(
        config: Config,
        meter_provider: AnyMeterProvider,
        pool: Pool,
        async_storage: Arc<AsyncStorage>,
        cdn: Option<Cdn>,
    ) -> Result<Self> {
        let config = Arc::new(config);

        let cdn = cdn.map(Arc::new);
        let async_build_queue = Arc::new(AsyncBuildQueue::new(
            pool.clone(),
            config.build_queue.clone(),
            &meter_provider,
        ));

        let runtime = runtime::Handle::current();

        // sync wrappers around build-queue & storage async resources
        let build_queue = Arc::new(BuildQueue::new(runtime.clone(), async_build_queue.clone()));
        let storage = Arc::new(Storage::new(async_storage.clone(), runtime.clone()));

        Ok(Self {
            async_build_queue,
            build_queue,
            builder_metrics: Arc::new(BuilderMetrics::new(&meter_provider)),
            storage,
            async_storage,
            cdn,
            pool: pool.clone(),
            registry_api: Arc::new(RegistryApi::from_config(&config.registry_api)?),
            repository_stats_updater: Arc::new(RepositoryStatsUpdater::new(
                &config.repository_stats,
                pool,
            )),
            runtime,
            config,
            meter_provider,
        })
    }
}
