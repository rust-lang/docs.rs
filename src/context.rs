use crate::cdn::CdnBackend;
use crate::db::Pool;
use crate::error::Result;
use crate::repositories::RepositoryStatsUpdater;
use crate::{
    AsyncBuildQueue, AsyncStorage, BuildQueue, Config, Index, InstanceMetrics, RegistryApi,
    ServiceMetrics, Storage,
};
use std::{future::Future, sync::Arc};
use tokio::runtime::Runtime;

pub trait Context {
    fn config(&self) -> Result<Arc<Config>>;
    fn async_build_queue(&self) -> impl Future<Output = Result<Arc<AsyncBuildQueue>>> + Send;
    fn build_queue(&self) -> Result<Arc<BuildQueue>>;
    fn storage(&self) -> Result<Arc<Storage>>;
    fn async_storage(&self) -> impl Future<Output = Result<Arc<AsyncStorage>>> + Send;
    fn cdn(&self) -> impl Future<Output = Result<Arc<CdnBackend>>> + Send;
    fn pool(&self) -> Result<Pool>;
    fn async_pool(&self) -> impl Future<Output = Result<Pool>> + Send;
    fn service_metrics(&self) -> Result<Arc<ServiceMetrics>>;
    fn instance_metrics(&self) -> Result<Arc<InstanceMetrics>>;
    fn index(&self) -> Result<Arc<Index>>;
    fn registry_api(&self) -> Result<Arc<RegistryApi>>;
    fn repository_stats_updater(&self) -> Result<Arc<RepositoryStatsUpdater>>;
    fn runtime(&self) -> Result<Arc<Runtime>>;
}
