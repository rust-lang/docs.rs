use crate::cdn::CdnBackend;
use crate::db::Pool;
use crate::repositories::RepositoryStatsUpdater;
use crate::{
    AsyncBuildQueue, AsyncStorage, BuildQueue, Config, Index, InstanceMetrics, RegistryApi,
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
    pub index: Arc<Index>,
    pub registry_api: Arc<RegistryApi>,
    pub repository_stats_updater: Arc<RepositoryStatsUpdater>,
    pub runtime: Arc<runtime::Runtime>,
}

impl Context {
    pub fn from_config(config: Config) -> Result<Self> {
        let config = Arc::new(config);
        let runtime = Arc::new(runtime::Builder::new_multi_thread().enable_all().build()?);

        let instance_metrics = Arc::new(InstanceMetrics::new()?);

        let pool = Pool::new(&config, runtime.clone(), instance_metrics.clone())?;
        let async_storage = Arc::new(runtime.block_on(AsyncStorage::new(
            pool.clone(),
            instance_metrics.clone(),
            config.clone(),
        ))?);

        let async_build_queue = Arc::new(AsyncBuildQueue::new(
            pool.clone(),
            instance_metrics.clone(),
            config.clone(),
            async_storage.clone(),
        ));

        let build_queue = Arc::new(BuildQueue::new(runtime.clone(), async_build_queue.clone()));

        let storage = Arc::new(Storage::new(async_storage.clone(), runtime.clone()));

        let cdn = Arc::new(runtime.block_on(CdnBackend::new(&config)));

        let index = Arc::new({
            let path = config.registry_index_path.clone();
            if let Some(registry_url) = config.registry_url.clone() {
                Index::from_url(path, registry_url)
            } else {
                Index::new(path)
            }?
        });

        Ok(Self {
            async_build_queue,
            build_queue,
            storage,
            async_storage,
            cdn,
            pool: pool.clone(),
            service_metrics: Arc::new(ServiceMetrics::new()?),
            instance_metrics,
            index,
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
