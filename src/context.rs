use crate::cdn::CdnBackend;
use crate::db::Pool;
use crate::repositories::RepositoryStatsUpdater;
use crate::{
    AsyncBuildQueue, AsyncStorage, BuildQueue, Config, Index, InstanceMetrics, RegistryApi,
    ServiceMetrics, Storage,
};
use bon::Builder;
use std::sync::Arc;
use tokio::runtime::Runtime;

#[derive(Builder)]
pub struct Context {
    pub config: Arc<Config>,
    pub async_build_queue: Arc<AsyncBuildQueue>,
    pub build_queue: Arc<BuildQueue>,
    pub storage: Arc<Storage>,
    pub async_storage: Arc<AsyncStorage>,
    pub cdn: Arc<CdnBackend>,
    pub pool: Pool,
    pub async_pool: Pool,
    pub service_metrics: Arc<ServiceMetrics>,
    pub instance_metrics: Arc<InstanceMetrics>,
    pub index: Arc<Index>,
    pub registry_api: Arc<RegistryApi>,
    pub repository_stats_updater: Arc<RepositoryStatsUpdater>,
    pub runtime: Arc<Runtime>,
}
