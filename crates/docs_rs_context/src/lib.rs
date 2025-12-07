use anyhow::{Result, anyhow};
use docs_rs_build_queue::{AsyncBuildQueue, Config};
use docs_rs_database::Pool;
use docs_rs_opentelemetry::{AnyMeterProvider, get_meter_provider};
use std::sync::Arc;

pub struct Context {
    meter_provider: AnyMeterProvider,
    pool: Option<Pool>,
    build_queue: Option<Arc<AsyncBuildQueue>>,
}

// builder
impl Context {
    pub fn new() -> Result<Self> {
        let config = docs_rs_opentelemetry::Config::from_environment()?;
        Ok(Context {
            meter_provider: get_meter_provider(&config)?,
            pool: None,
            build_queue: None,
        })
    }

    pub async fn with_pool(mut self) -> Result<Self> {
        if self.pool.is_some() {
            return Ok(self);
        }

        let config = docs_rs_database::Config::from_environment()?;
        let pool = Pool::new(&config, &self.meter_provider).await?;
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
        let build_queue = AsyncBuildQueue::new(pool, &config, &self.meter_provider);
        self.build_queue = Some(Arc::new(build_queue));
        Ok(self)
    }
}

// accessors
impl Context {
    pub fn meter_provider(&self) -> &AnyMeterProvider {
        &self.meter_provider
    }

    pub fn pool(&self) -> Result<Pool> {
        if let Some(ref pool) = self.pool {
            Ok(pool.clone())
        } else {
            Err(anyhow!("Pool is not initialized"))
        }
    }

    pub fn build_queue(&self) -> Result<Arc<AsyncBuildQueue>> {
        if let Some(ref build_queue) = self.build_queue {
            Ok(build_queue.clone())
        } else {
            Err(anyhow!("Build queue is not initialized"))
        }
    }
}
