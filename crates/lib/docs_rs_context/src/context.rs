use crate::config::Config;
use anyhow::{Result, anyhow, bail};
use docs_rs_build_queue::{AsyncBuildQueue, BuildQueue};
use docs_rs_config::AppConfig as _;
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
    finish_fn(name = build_internal, vis = "",),
    on(_, into)
)]
pub struct Context {
    #[builder(field)]
    pub config: Config,

    #[builder(getter)]
    pub runtime: runtime::Handle,

    #[builder(getter)]
    pub meter_provider: AnyMeterProvider,

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

use context_builder::{
    IsComplete, IsSet, IsUnset, SetBlockingBuildQueue, SetBlockingStorage, SetBuildQueue, SetCdn,
    SetMeterProvider, SetPool, SetRegistryApi, SetRepositoryStats, SetRuntime, SetStorage, State,
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

    pub async fn with_runtime(self) -> Result<ContextBuilder<SetRuntime<S>>>
    where
        S::Runtime: IsUnset,
    {
        Ok(self.runtime(runtime::Handle::try_current()?))
    }

    pub fn with_meter_provider(self) -> Result<ContextBuilder<SetMeterProvider<S>>>
    where
        S::MeterProvider: IsUnset,
    {
        Ok(self.meter_provider(get_meter_provider(
            &docs_rs_opentelemetry::Config::from_environment()?,
        )?))
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
        S::MeterProvider: IsSet,
        S::Pool: IsUnset,
    {
        let config = Arc::new(docs_rs_database::Config::from_environment()?);
        let pool = Pool::new(&config, self.get_meter_provider()).await?;
        Ok(self.pool(config, pool))
    }

    pub fn storage(
        mut self,
        config: Arc<docs_rs_storage::Config>,
        storage: Arc<AsyncStorage>,
    ) -> ContextBuilder<SetBlockingStorage<SetStorage<S>>>
    where
        S::Runtime: IsSet,
        S::Storage: IsUnset,
        S::BlockingStorage: IsUnset,
    {
        self.config.storage = Some(config);
        let blocking_storage = Arc::new(Storage::new(storage.clone(), self.get_runtime().clone()));
        self.storage_internal(storage.clone())
            .blocking_storage_internal(blocking_storage)
    }

    pub async fn with_storage(self) -> Result<ContextBuilder<SetBlockingStorage<SetStorage<S>>>>
    where
        S::Runtime: IsSet,
        S::MeterProvider: IsSet,
        S::Storage: IsUnset,
        S::BlockingStorage: IsUnset,
    {
        let config = Arc::new(docs_rs_storage::Config::from_environment()?);
        let storage = Arc::new(AsyncStorage::new(config.clone(), self.get_meter_provider()).await?);
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

    pub fn with_maybe_cdn(self) -> Result<ContextBuilder<SetCdn<S>>>
    where
        S::MeterProvider: IsSet,
        S::Cdn: IsUnset,
    {
        let config = Arc::new(docs_rs_fastly::Config::from_environment()?);

        let cdn = if config.is_valid() {
            Some(Arc::new(Cdn::from_config(
                &config,
                self.get_meter_provider(),
            )?))
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
        S::Runtime: IsSet,
        S::BuildQueue: IsUnset,
        S::BlockingBuildQueue: IsUnset,
    {
        self.config.build_queue = Some(config);
        let blocking_build_queue = BuildQueue::new(self.get_runtime().clone(), build_queue.clone());
        self.build_queue_internal(build_queue.clone())
            .blocking_build_queue_internal(Arc::new(blocking_build_queue))
    }

    pub fn with_build_queue(self) -> Result<ContextBuilder<SetBlockingBuildQueue<SetBuildQueue<S>>>>
    where
        S::Runtime: IsSet,
        S::MeterProvider: IsSet,
        S::Pool: IsSet,
        S::BuildQueue: IsUnset,
        S::BlockingBuildQueue: IsUnset,
    {
        let pool = self.get_pool().expect("pool is set");

        let config = Arc::new(docs_rs_build_queue::Config::from_environment()?);
        let build_queue = Arc::new(AsyncBuildQueue::new(
            pool.clone(),
            config.clone(),
            self.get_meter_provider(),
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

    pub fn with_registry_api(self) -> Result<ContextBuilder<SetRegistryApi<S>>>
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

    pub fn with_repository_stats(self) -> Result<ContextBuilder<SetRepositoryStats<S>>>
    where
        S::Pool: IsSet,
        S::RepositoryStats: IsUnset,
    {
        let pool = self.get_pool().expect("pool is set");

        let config = Arc::new(docs_rs_repository_stats::Config::from_environment()?);
        let updater = RepositoryStatsUpdater::new(&config, pool.clone());

        Ok(self.repository_stats(config, updater.into()))
    }

    pub fn build_limits(mut self, config: Arc<docs_rs_build_limits::Config>) -> ContextBuilder<S> {
        self.config.build_limits = Some(config);
        self
    }

    pub fn with_build_limits(self) -> Result<ContextBuilder<S>> {
        let config = docs_rs_build_limits::Config::from_environment()?;
        Ok(self.build_limits(config.into()))
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

    /// return configured CDN or None.
    ///
    /// Compared to other parts of the context, the CDN is truly optional
    /// and all code using it must handle the None case.
    pub fn cdn(&self) -> Option<&Arc<Cdn>> {
        self.cdn.as_ref()
    }

    pub fn repository_stats(&self) -> Result<&Arc<RepositoryStatsUpdater>> {
        if let Some(ref updater) = self.repository_stats {
            Ok(updater)
        } else {
            Err(anyhow!("Repository stats updater is not initialized"))
        }
    }
}
