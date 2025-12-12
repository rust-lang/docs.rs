use anyhow::{Result, anyhow};
use docs_rs_build_queue::{AsyncBuildQueue, BuildQueue};
use docs_rs_database::Pool;
use docs_rs_fastly::Cdn;
use docs_rs_opentelemetry::{AnyMeterProvider, get_meter_provider};
use docs_rs_registry_api::RegistryApi;
use docs_rs_storage::{AsyncStorage, Storage};
use std::sync::Arc;
use tokio::runtime::Handle;

#[derive(Debug, Default)]
pub struct Config {
    opentelemetry: Option<Arc<docs_rs_opentelemetry::Config>>,
    build_queue: Option<Arc<docs_rs_build_queue::Config>>,
    database: Option<Arc<docs_rs_database::Config>>,
    storage: Option<Arc<docs_rs_storage::Config>>,
    registry_api: Option<Arc<docs_rs_registry_api::Config>>,
    cdn: Option<Arc<docs_rs_fastly::Config>>,
}

pub struct Context {
    meter_provider: AnyMeterProvider,

    pool: Option<Pool>,

    build_queue: Option<Arc<AsyncBuildQueue>>,
    blocking_build_queue: Option<Arc<BuildQueue>>,

    storage: Option<Arc<AsyncStorage>>,
    blocking_storage: Option<Arc<docs_rs_storage::Storage>>,

    registry_api: Option<Arc<RegistryApi>>,

    cdn: Option<Arc<Cdn>>,

    runtime: Handle,
    config: Config,
}

// builder
impl Context {
    pub fn new() -> Result<Self> {
        Self::new_with_runtime(Handle::try_current()?)
    }

    pub fn new_with_runtime(runtime: Handle) -> Result<Self> {
        let config = docs_rs_opentelemetry::Config::from_environment()?;
        Ok(Context {
            meter_provider: get_meter_provider(&config)?,
            runtime,
            config: Config {
                opentelemetry: Some(Arc::new(config)),
                ..Default::default()
            },
            pool: None,
            build_queue: None,
            blocking_build_queue: None,
            storage: None,
            blocking_storage: None,
            registry_api: None,
            cdn: None,
        })
    }

    pub async fn with_pool(mut self) -> Result<Self> {
        if self.pool.is_some() {
            return Ok(self);
        }

        let config = docs_rs_database::Config::from_environment()?;
        let pool = Pool::new(&config, &self.meter_provider).await?;
        self.config.database = Some(Arc::new(config));
        self.pool = Some(pool);
        Ok(self)
    }

    pub async fn with_build_queue(mut self) -> Result<Self> {
        if self.build_queue.is_some() {
            return Ok(self);
        }

        self = self.with_pool().await?;

        let pool = self.pool()?;

        let config = docs_rs_build_queue::Config::from_environment()?;
        let build_queue = Arc::new(AsyncBuildQueue::new(pool, &config, &self.meter_provider));
        let blocking_build_queue =
            Arc::new(BuildQueue::new(self.runtime.clone(), build_queue.clone()));

        self.config.build_queue = Some(Arc::new(config));
        self.build_queue = Some(build_queue);
        self.blocking_build_queue = Some(blocking_build_queue);
        Ok(self)
    }

    pub async fn with_storage(mut self) -> Result<Self> {
        if self.storage.is_some() {
            return Ok(self);
        }

        self = self.with_pool().await?;
        let pool = self.pool()?;

        let config = Arc::new(docs_rs_storage::Config::from_environment()?);
        let storage =
            Arc::new(AsyncStorage::new(pool, config.clone(), &self.meter_provider).await?);
        self.config.storage = Some(config);
        self.storage = Some(storage);
        Ok(self)
    }

    pub async fn with_registry_api(mut self) -> Result<Self> {
        if self.registry_api.is_some() {
            return Ok(self);
        }

        let config = docs_rs_registry_api::Config::from_environment()?;
        let api = RegistryApi::from_config(&config)?;

        self.registry_api = Some(Arc::new(api));
        self.config.registry_api = Some(Arc::new(config));
        Ok(self)
    }

    pub async fn with_cdn(mut self) -> Result<Self> {
        if self.cdn.is_some() {
            return Ok(self);
        }

        let config = Arc::new(docs_rs_fastly::Config::from_environment()?);
        let cdn = Cdn::from_config(config.clone(), &self.meter_provider)?;

        self.cdn = Some(Arc::new(cdn));
        self.config.cdn = Some(config);
        Ok(self)
    }
}

// accessors
impl Context {
    pub fn meter_provider(&self) -> &AnyMeterProvider {
        &self.meter_provider
    }

    pub fn runtime(&self) -> &Handle {
        &self.runtime
    }

    pub fn pool(&self) -> Result<Pool> {
        if let Some(ref pool) = self.pool {
            Ok(pool.clone())
        } else {
            Err(anyhow!("Pool is not initialized"))
        }
    }

    pub fn storage(&self) -> Result<Arc<AsyncStorage>> {
        if let Some(ref storage) = self.storage {
            Ok(storage.clone())
        } else {
            Err(anyhow!("Storage is not initialized"))
        }
    }

    pub fn blocking_storage(&self) -> Result<Arc<Storage>> {
        if let Some(ref storage) = self.blocking_storage {
            Ok(storage.clone())
        } else {
            Err(anyhow!("blocking Storage is not initialized"))
        }
    }

    pub fn build_queue(&self) -> Result<Arc<AsyncBuildQueue>> {
        if let Some(ref build_queue) = self.build_queue {
            Ok(build_queue.clone())
        } else {
            Err(anyhow!("Build queue is not initialized"))
        }
    }

    pub fn blocking_build_queue(&self) -> Result<Arc<BuildQueue>> {
        if let Some(ref build_queue) = self.blocking_build_queue {
            Ok(build_queue.clone())
        } else {
            Err(anyhow!("blocking Build queue is not initialized"))
        }
    }

    pub fn registry_api(&self) -> Result<Arc<RegistryApi>> {
        if let Some(ref registry_api) = self.registry_api {
            Ok(registry_api.clone())
        } else {
            Err(anyhow!("Registry API is not initialized"))
        }
    }

    pub fn cdn(&self) -> Result<Arc<Cdn>> {
        if let Some(ref cdn) = self.cdn {
            Ok(cdn.clone())
        } else {
            Err(anyhow!("CDN is not initialized"))
        }
    }
}
