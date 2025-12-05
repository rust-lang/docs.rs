use crate::{Config, RegistryApi, cdn::CdnMetrics};
use anyhow::Result;
use docs_rs_database::Pool;
use docs_rs_opentelemetry::{AnyMeterProvider, get_meter_provider};
use std::sync::Arc;
use tokio::runtime;

pub struct Context {
    pub config: Arc<Config>,
    pub storage: Arc<Storage>,
    pub async_storage: Arc<AsyncStorage>,
    pub cdn_metrics: Arc<CdnMetrics>,
    pub pool: Pool,
    pub registry_api: Arc<RegistryApi>,
    pub runtime: runtime::Handle,
    pub meter_provider: AnyMeterProvider,
}

impl Context {
    /// Create a new context environment from the given configuration.
    pub async fn from_config(config: Config) -> Result<Self> {
        let meter_provider = get_meter_provider(&config)?;
        let pool = Pool::new(&config, &meter_provider).await?;
        Self::from_config_with_metrics_and_pool(config, meter_provider, pool).await
    }

    /// Create a new context environment from the given configuration, for running tests.
    #[cfg(test)]
    pub async fn from_test_config(
        config: Config,
        meter_provider: AnyMeterProvider,
        pool: Pool,
    ) -> Result<Self> {
        Self::from_config_with_metrics_and_pool(config, meter_provider, pool).await
    }

    /// private function for context environment generation, allows passing in a
    /// preconfigured instance metrics & pool from the database.
    /// Mostly so we can support test environments with their db
    async fn from_config_with_metrics_and_pool(
        config: Config,
        meter_provider: AnyMeterProvider,
        pool: Pool,
    ) -> Result<Self> {
        let config = Arc::new(config);

        let async_storage =
            Arc::new(AsyncStorage::new(pool.clone(), config.clone(), &meter_provider).await?);

        let cdn_metrics = Arc::new(CdnMetrics::new(&meter_provider));
        let async_build_queue = Arc::new(AsyncBuildQueue::new(
            pool.clone(),
            config.clone(),
            async_storage.clone(),
            cdn_metrics.clone(),
            &meter_provider,
        ));

        let runtime = runtime::Handle::current();

        // sync wrappers around build-queue & storage async resources
        let build_queue = Arc::new(BuildQueue::new(runtime.clone(), async_build_queue.clone()));
        let storage = Arc::new(Storage::new(async_storage.clone(), runtime.clone()));

        Ok(Self {
            // async_build_queue,
            // build_queue,
            storage,
            async_storage,
            cdn_metrics,
            pool: pool.clone(),
            registry_api: Arc::new(RegistryApi::new(
                config.registry_api_host.clone(),
                config.crates_io_api_call_retries,
            )?),
            // repository_stats_updater: Arc::new(RepositoryStatsUpdater::new(&config, pool)),
            runtime,
            config,
            meter_provider,
        })
    }
}
