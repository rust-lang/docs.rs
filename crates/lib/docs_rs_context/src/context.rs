use crate::config::Config;
use anyhow::{Result, anyhow, bail};
use docs_rs_build_queue::{AsyncBuildQueue, BuildQueue};
use docs_rs_database::Pool;
use docs_rs_fastly::Cdn;
use docs_rs_opentelemetry::{AnyMeterProvider, get_meter_provider};
use docs_rs_registry_api::RegistryApi;
use docs_rs_repository_stats::RepositoryStatsUpdater;
use docs_rs_storage::{AsyncStorage, Storage};
use std::sync::Arc;
use tokio::runtime;

#[derive(bon::Builder)]
#[builder(
    start_fn(name = builder_internal, vis = "",),
    finish_fn(name = build_internal, vis = "",),
    on(_, into)
)]
pub struct Context {
    #[builder(start_fn)]
    pub runtime: runtime::Handle,

    #[builder(start_fn)]
    pub meter_provider: AnyMeterProvider,

    #[builder(field)]
    pub config: Config,

    #[builder(getter, setters(vis = "", name = pool_internal))]
    pub pool: Option<Pool>,

    #[builder(setters(vis = "", name = build_queue_internal))]
    pub build_queue: Option<Arc<AsyncBuildQueue>>,
    #[builder(setters(vis = "", name = blocking_build_queue_internal))]
    pub blocking_build_queue: Option<Arc<BuildQueue>>,

    #[builder(setters(vis = "", name = storage_internal))]
    pub storage: Option<Arc<AsyncStorage>>,
    #[builder(setters(vis = "", name = blocking_storage_internal))]
    pub blocking_storage: Option<Arc<docs_rs_storage::Storage>>,

    #[builder(setters(vis = "", name = registry_api_internal))]
    pub registry_api: Option<Arc<RegistryApi>>,

    #[builder(setters(vis = "", name = cdn_internal))]
    pub cdn: Option<Arc<Cdn>>,

    #[builder(setters(vis = "", name = repository_stats_internal))]
    pub repository_stats: Option<Arc<RepositoryStatsUpdater>>,
}

// builder
impl Context {
    pub async fn builder() -> Result<ContextBuilder> {
        // this method is async to make it clear to the caller that
        // it needs the runtime context.
        Context::builder_with_runtime(runtime::Handle::try_current()?)
    }

    pub fn builder_with_runtime(runtime: runtime::Handle) -> Result<ContextBuilder> {
        Ok(Context::builder_internal(
            runtime,
            get_meter_provider(&docs_rs_opentelemetry::Config::from_environment()?)?,
        ))
    }
}

use context_builder::{
    IsComplete, IsSet, IsUnset, SetBlockingBuildQueue, SetBlockingStorage, SetBuildQueue, SetCdn,
    SetPool, SetRegistryApi, SetRepositoryStats, SetStorage, State,
};

impl<S: State> ContextBuilder<S> {
    pub fn build(self) -> Result<Context>
    where
        S: IsComplete,
    {
        let ctx = self.build_internal();

        if !(ctx.config().build_queue.is_some() == ctx.build_queue.is_some()
            && ctx.build_queue.is_some() == ctx.blocking_build_queue.is_some())
        {
            bail!("build_queue config and instance mismatch");
        }

        if ctx.config().database.is_some() != ctx.pool.is_some() {
            bail!("database/pool config and instance mismatch");
        }

        if !(ctx.config().storage.is_some() == ctx.storage.is_some()
            && ctx.config().storage.is_some() == ctx.blocking_storage.is_some())
        {
            bail!("storage config and instance mismatch");
        }

        if ctx.config().registry_api.is_some() != ctx.registry_api.is_some() {
            bail!("registry_api config and instance mismatch");
        }

        if ctx.cdn.is_some() && ctx.config().cdn.is_none() {
            // NOTE: slightly different check.
            // for CDN we check the config if it's a usable one,
            // and set `None` if not.
            bail!("cdn config and instance mismatch");
        }

        if ctx.config().repository_stats.is_some() != ctx.repository_stats.is_some() {
            bail!("repository_stats config and instance mismatch");
        }

        Ok(ctx)
    }

    fn meter_provider(&self) -> &AnyMeterProvider {
        &self.meter_provider
    }

    pub fn pool(
        mut self,
        config: Arc<docs_rs_database::Config>,
        pool: Pool,
    ) -> ContextBuilder<SetPool<S>>
    where
        S::Pool: IsUnset,
    {
        self.config.database = Some(config);
        self.pool_internal(pool)
    }

    pub async fn with_pool(self) -> Result<ContextBuilder<SetPool<S>>>
    where
        S::Pool: IsUnset,
    {
        let config = Arc::new(docs_rs_database::Config::from_environment()?);
        let pool = Pool::new(&config, self.meter_provider()).await?;
        Ok(self.pool(config, pool))
    }

    pub fn storage(
        mut self,
        config: Arc<docs_rs_storage::Config>,
        storage: Arc<AsyncStorage>,
    ) -> ContextBuilder<SetBlockingStorage<SetStorage<S>>>
    where
        S::Storage: IsUnset,
        S::BlockingStorage: IsUnset,
    {
        self.config.storage = Some(config);
        let blocking_storage = Arc::new(Storage::new(storage.clone(), self.runtime.clone()));
        self.storage_internal(storage.clone())
            .blocking_storage_internal(blocking_storage)
    }

    pub async fn with_storage(self) -> Result<ContextBuilder<SetBlockingStorage<SetStorage<S>>>>
    where
        S::Storage: IsUnset,
        S::BlockingStorage: IsUnset,
    {
        let config = Arc::new(docs_rs_storage::Config::from_environment()?);
        let storage = Arc::new(AsyncStorage::new(config.clone(), self.meter_provider()).await?);
        Ok(self.storage(config, storage))
    }

    pub fn maybe_cdn(
        mut self,
        config: Arc<docs_rs_fastly::Config>,
        cdn: Option<Arc<Cdn>>,
    ) -> ContextBuilder<SetCdn<S>>
    where
        S::Cdn: IsUnset,
    {
        self.config.cdn = Some(config);
        self.maybe_cdn_internal(cdn)
    }

    pub async fn with_cdn(self) -> Result<ContextBuilder<SetCdn<S>>>
    where
        S::Cdn: IsUnset,
    {
        let config = Arc::new(docs_rs_fastly::Config::from_environment()?);

        let cdn = if config.is_valid() {
            Some(Arc::new(Cdn::from_config(&config, self.meter_provider())?))
        } else {
            None
        };

        Ok(self.maybe_cdn(config, cdn))
    }

    pub fn build_queue(
        mut self,
        config: Arc<docs_rs_build_queue::Config>,
        build_queue: Arc<AsyncBuildQueue>,
    ) -> ContextBuilder<SetBlockingBuildQueue<SetBuildQueue<S>>>
    where
        S::BuildQueue: IsUnset,
        S::BlockingBuildQueue: IsUnset,
    {
        self.config.build_queue = Some(config);
        let blocking_build_queue = BuildQueue::new(self.runtime.clone(), build_queue.clone());
        self.build_queue_internal(build_queue.clone())
            .blocking_build_queue_internal(Arc::new(blocking_build_queue))
    }

    pub async fn with_build_queue(
        self,
    ) -> Result<ContextBuilder<SetBlockingBuildQueue<SetBuildQueue<S>>>>
    where
        S::Pool: IsSet,
        S::BuildQueue: IsUnset,
        S::BlockingBuildQueue: IsUnset,
    {
        let pool = self.get_pool().expect("pool is set");

        let config = Arc::new(docs_rs_build_queue::Config::from_environment()?);
        let build_queue = Arc::new(AsyncBuildQueue::new(
            pool.clone(),
            config.clone(),
            self.meter_provider(),
        ));

        Ok(self.build_queue(config, build_queue))
    }

    pub fn registry_api(
        mut self,
        config: Arc<docs_rs_registry_api::Config>,
        registry_api: Arc<RegistryApi>,
    ) -> ContextBuilder<SetRegistryApi<S>>
    where
        S::RegistryApi: IsUnset,
    {
        self.config.registry_api = Some(config);
        self.registry_api_internal(registry_api)
    }

    pub async fn with_registry_api(self) -> Result<ContextBuilder<SetRegistryApi<S>>>
    where
        S::RegistryApi: IsUnset,
    {
        let config = docs_rs_registry_api::Config::from_environment()?;
        let api = RegistryApi::from_config(&config)?;

        Ok(self.registry_api(config.into(), api.into()))
    }

    pub fn repository_stats(
        mut self,
        config: Arc<docs_rs_repository_stats::Config>,
        repository_stats: Arc<RepositoryStatsUpdater>,
    ) -> ContextBuilder<SetRepositoryStats<S>>
    where
        S::RepositoryStats: IsUnset,
    {
        self.config.repository_stats = Some(config);
        self.repository_stats_internal(repository_stats)
    }

    pub async fn with_repository_stats(self) -> Result<ContextBuilder<SetRepositoryStats<S>>>
    where
        S::Pool: IsSet,
        S::RepositoryStats: IsUnset,
    {
        let pool = self.get_pool().expect("pool is set");

        let config = Arc::new(docs_rs_repository_stats::Config::from_environment()?);
        let updater = RepositoryStatsUpdater::new(&config, pool.clone());

        Ok(self.repository_stats(config, updater.into()))
    }
}

// accessors
impl Context {
    pub fn meter_provider(&self) -> &AnyMeterProvider {
        &self.meter_provider
    }

    pub fn runtime(&self) -> &runtime::Handle {
        &self.runtime
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn pool(&self) -> Result<&Pool> {
        if let Some(ref pool) = self.pool {
            Ok(pool)
        } else {
            Err(anyhow!("Pool is not initialized"))
        }
    }

    pub fn storage(&self) -> Result<&Arc<AsyncStorage>> {
        if let Some(ref storage) = self.storage {
            Ok(storage)
        } else {
            Err(anyhow!("Storage is not initialized"))
        }
    }

    pub fn blocking_storage(&self) -> Result<&Arc<Storage>> {
        if let Some(ref storage) = self.blocking_storage {
            Ok(storage)
        } else {
            Err(anyhow!("blocking Storage is not initialized"))
        }
    }

    pub fn build_queue(&self) -> Result<&Arc<AsyncBuildQueue>> {
        if let Some(ref build_queue) = self.build_queue {
            Ok(build_queue)
        } else {
            Err(anyhow!("Build queue is not initialized"))
        }
    }

    pub fn blocking_build_queue(&self) -> Result<&Arc<BuildQueue>> {
        if let Some(ref build_queue) = self.blocking_build_queue {
            Ok(build_queue)
        } else {
            Err(anyhow!("blocking Build queue is not initialized"))
        }
    }

    pub fn registry_api(&self) -> Result<&Arc<RegistryApi>> {
        if let Some(ref registry_api) = self.registry_api {
            Ok(registry_api)
        } else {
            Err(anyhow!("Registry API is not initialized"))
        }
    }

    pub fn cdn(&self) -> Result<&Arc<Cdn>> {
        if let Some(ref cdn) = self.cdn {
            Ok(cdn)
        } else {
            Err(anyhow!("CDN is not initialized"))
        }
    }

    pub fn repository_stats(&self) -> Result<&Arc<RepositoryStatsUpdater>> {
        if let Some(ref updater) = self.repository_stats {
            Ok(updater)
        } else {
            Err(anyhow!("Repository stats updater is not initialized"))
        }
    }
}
