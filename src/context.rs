use crate::cdn::CdnBackend;
use crate::db::Pool;
use crate::repositories::RepositoryStatsUpdater;
use crate::{
    AsyncBuildQueue, AsyncStorage, BuildQueue, Config, InstanceMetrics, RegistryApi,
    ServiceMetrics, Storage,
};
use anyhow::Result;
use std::sync::Arc;
use tokio::runtime;

pub struct Context {
    pub config: Arc<Config>,
    pub async_build_queue: Arc<AsyncBuildQueue>,
    pub build_queue: Arc<BuildQueue>,
    pub storage: Arc<Storage>,
    pub async_storage: Arc<AsyncStorage>,
    pub cdn: Arc<CdnBackend>,
    pub pool: Pool,
    pub service_metrics: Arc<ServiceMetrics>,
    pub instance_metrics: Arc<InstanceMetrics>,
    pub registry_api: Arc<RegistryApi>,
    pub repository_stats_updater: Arc<RepositoryStatsUpdater>,
    pub runtime: runtime::Handle,
}

impl Context {
    /// Create a new context environment from the given configuration.
    #[cfg(not(test))]
    pub async fn from_config(config: Config) -> Result<Self> {
        let instance_metrics = Arc::new(InstanceMetrics::new()?);
        let pool = Pool::new(&config, instance_metrics.clone()).await?;
        Self::from_config_with_metrics_and_pool(config, instance_metrics, pool).await
    }

    /// Create a new context environment from the given configuration, for running tests.
    #[cfg(test)]
    pub async fn from_config(
        config: Config,
        instance_metrics: Arc<InstanceMetrics>,
        pool: Pool,
    ) -> Result<Self> {
        Self::from_config_with_metrics_and_pool(config, instance_metrics, pool).await
    }

    /// private function for context environment generation, allows passing in a
    /// preconfigured instance metrics & pool from the database.
    /// Mostly so we can support test environments with their db
    async fn from_config_with_metrics_and_pool(
        config: Config,
        instance_metrics: Arc<InstanceMetrics>,
        pool: Pool,
    ) -> Result<Self> {
        let config = Arc::new(config);

        let async_storage = Arc::new(
            AsyncStorage::new(pool.clone(), instance_metrics.clone(), config.clone()).await?,
        );

        let async_build_queue = Arc::new(AsyncBuildQueue::new(
            pool.clone(),
            instance_metrics.clone(),
            config.clone(),
            async_storage.clone(),
        ));

        let cdn = Arc::new(CdnBackend::new(&config).await);

        let runtime = runtime::Handle::current();
        // sync wrappers around build-queue & storage async resources
        let build_queue = Arc::new(BuildQueue::new(runtime.clone(), async_build_queue.clone()));
        let storage = Arc::new(Storage::new(async_storage.clone(), runtime.clone()));

        Ok(Self {
            async_build_queue,
            build_queue,
            storage,
            async_storage,
            cdn,
            pool: pool.clone(),
            service_metrics: Arc::new(ServiceMetrics::new()?),
            instance_metrics,
            registry_api: Arc::new(RegistryApi::new(
                config.registry_api_host.clone(),
                config.crates_io_api_call_retries,
            )?),
            repository_stats_updater: Arc::new(RepositoryStatsUpdater::new(&config, pool)),
            runtime,
            config,
        })
    }
}
