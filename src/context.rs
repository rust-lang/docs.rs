use crate::cdn::CdnBackend;
use crate::db::Pool;
use crate::error::Result;
use crate::repositories::RepositoryStatsUpdater;
use crate::{
    AsyncStorage, BuildQueue, Config, Index, InstanceMetrics, RegistryApi, ServiceMetrics, Storage,
};
use axum::async_trait;
use std::sync::Arc;
use tokio::runtime::Runtime;

#[async_trait]
pub trait Context {
    fn config(&self) -> Result<Arc<Config>>;
    fn build_queue(&self) -> Result<Arc<BuildQueue>>;
    fn storage(&self) -> Result<Arc<Storage>>;
    async fn async_storage(&self) -> Result<Arc<AsyncStorage>>;
    fn cdn(&self) -> Result<Arc<CdnBackend>>;
    fn pool(&self) -> Result<Pool>;
    fn service_metrics(&self) -> Result<Arc<ServiceMetrics>>;
    fn instance_metrics(&self) -> Result<Arc<InstanceMetrics>>;
    fn index(&self) -> Result<Arc<Index>>;
    fn registry_api(&self) -> Result<Arc<RegistryApi>>;
    fn repository_stats_updater(&self) -> Result<Arc<RepositoryStatsUpdater>>;
    fn runtime(&self) -> Result<Arc<Runtime>>;
}
