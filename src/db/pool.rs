use crate::metrics::InstanceMetrics;
use crate::Config;
use postgres::{Client, NoTls};
use r2d2_postgres::PostgresConnectionManager;
use std::sync::Arc;
use tracing::debug;

pub type PoolClient = r2d2::PooledConnection<PostgresConnectionManager<NoTls>>;

const DEFAULT_SCHEMA: &str = "public";

#[derive(Debug, Clone)]
pub struct Pool {
    #[cfg(test)]
    pool: Arc<std::sync::Mutex<Option<r2d2::Pool<PostgresConnectionManager<NoTls>>>>>,
    #[cfg(not(test))]
    pool: r2d2::Pool<PostgresConnectionManager<NoTls>>,
    metrics: Arc<InstanceMetrics>,
    max_size: u32,
}

impl Pool {
    pub fn new(config: &Config, metrics: Arc<InstanceMetrics>) -> Result<Pool, PoolError> {
        debug!(
            "creating database pool (if this hangs, consider running `docker-compose up -d db s3`)"
        );
        Self::new_inner(config, metrics, DEFAULT_SCHEMA)
    }

    #[cfg(test)]
    pub(crate) fn new_with_schema(
        config: &Config,
        metrics: Arc<InstanceMetrics>,
        schema: &str,
    ) -> Result<Pool, PoolError> {
        Self::new_inner(config, metrics, schema)
    }

    fn new_inner(
        config: &Config,
        metrics: Arc<InstanceMetrics>,
        schema: &str,
    ) -> Result<Pool, PoolError> {
        let url = config
            .database_url
            .parse()
            .map_err(PoolError::InvalidDatabaseUrl)?;
        let manager = PostgresConnectionManager::new(url, NoTls);
        let pool = r2d2::Pool::builder()
            .max_size(config.max_pool_size)
            .min_idle(Some(config.min_pool_idle))
            .connection_customizer(Box::new(SetSchema::new(schema)))
            .build(manager)
            .map_err(PoolError::PoolCreationFailed)?;

        Ok(Pool {
            #[cfg(test)]
            pool: Arc::new(std::sync::Mutex::new(Some(pool))),
            #[cfg(not(test))]
            pool,
            metrics,
            max_size: config.max_pool_size,
        })
    }

    fn with_pool<R>(
        &self,
        f: impl FnOnce(&r2d2::Pool<PostgresConnectionManager<NoTls>>) -> R,
    ) -> R {
        #[cfg(test)]
        {
            f(self.pool.lock().unwrap().as_ref().unwrap())
        }
        #[cfg(not(test))]
        {
            f(&self.pool)
        }
    }

    pub fn get(&self) -> Result<PoolClient, PoolError> {
        match self.with_pool(|p| p.get()) {
            Ok(conn) => Ok(conn),
            Err(err) => {
                self.metrics.failed_db_connections.inc();
                Err(PoolError::ClientError(err))
            }
        }
    }

    pub(crate) fn used_connections(&self) -> u32 {
        self.with_pool(|p| p.state().connections - p.state().idle_connections)
    }

    pub(crate) fn idle_connections(&self) -> u32 {
        self.with_pool(|p| p.state().idle_connections)
    }

    pub(crate) fn max_size(&self) -> u32 {
        self.max_size
    }

    #[cfg(test)]
    pub(crate) fn shutdown(&self) {
        self.pool.lock().unwrap().take();
    }
}

#[derive(Debug)]
struct SetSchema {
    schema: String,
}

impl SetSchema {
    fn new(schema: &str) -> Self {
        Self {
            schema: schema.into(),
        }
    }
}

impl r2d2::CustomizeConnection<Client, postgres::Error> for SetSchema {
    fn on_acquire(&self, conn: &mut Client) -> Result<(), postgres::Error> {
        if self.schema != DEFAULT_SCHEMA {
            conn.execute(
                format!("SET search_path TO {}, {};", self.schema, DEFAULT_SCHEMA).as_str(),
                &[],
            )?;
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    #[error("the provided database URL was not valid")]
    InvalidDatabaseUrl(#[from] postgres::Error),

    #[error("failed to create the database connection pool")]
    PoolCreationFailed(#[source] r2d2::Error),

    #[error("failed to get a database connection")]
    ClientError(#[source] r2d2::Error),
}
