use crate::metrics::InstanceMetrics;
use crate::Config;
use futures_util::{future::BoxFuture, stream::BoxStream};
use postgres::{Client, NoTls};
use r2d2_postgres::PostgresConnectionManager;
use sqlx::{postgres::PgPoolOptions, Executor};
use std::sync::Arc;
use tokio::runtime::Runtime;
use tracing::debug;

pub type PoolClient = r2d2::PooledConnection<PostgresConnectionManager<NoTls>>;
pub type AsyncPoolClient = sqlx::pool::PoolConnection<sqlx::postgres::Postgres>;

const DEFAULT_SCHEMA: &str = "public";

#[derive(Debug, Clone)]
pub struct Pool {
    #[cfg(test)]
    pool: Arc<std::sync::Mutex<Option<r2d2::Pool<PostgresConnectionManager<NoTls>>>>>,
    #[cfg(not(test))]
    pool: r2d2::Pool<PostgresConnectionManager<NoTls>>,
    async_pool: sqlx::PgPool,
    metrics: Arc<InstanceMetrics>,
    max_size: u32,
}

impl Pool {
    pub fn new(
        config: &Config,
        runtime: &Runtime,
        metrics: Arc<InstanceMetrics>,
    ) -> Result<Pool, PoolError> {
        debug!(
            "creating database pool (if this hangs, consider running `docker-compose up -d db s3`)"
        );
        Self::new_inner(config, runtime, metrics, DEFAULT_SCHEMA)
    }

    #[cfg(test)]
    pub(crate) fn new_with_schema(
        config: &Config,
        runtime: &Runtime,
        metrics: Arc<InstanceMetrics>,
        schema: &str,
    ) -> Result<Pool, PoolError> {
        Self::new_inner(config, runtime, metrics, schema)
    }

    fn new_inner(
        config: &Config,
        runtime: &Runtime,
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

        let _guard = runtime.enter();
        let async_pool = PgPoolOptions::new()
            // FIXME: these pool sizes would have to be validated before the pool is used in
            // a production setting.
            // Currently we only use it for the storage DB backend, which is only used for local
            // & unit-testing.
            .max_connections(config.max_pool_size)
            // .min_connections(config.min_pool_idle)
            .after_connect({
                let schema = schema.to_owned();
                move |conn, _meta| {
                    Box::pin({
                        let schema = schema.clone();

                        async move {
                            if schema != DEFAULT_SCHEMA {
                                conn.execute(
                                    format!("SET search_path TO {}, {};", schema, DEFAULT_SCHEMA)
                                        .as_str(),
                                )
                                .await?;
                            }

                            Ok(())
                        }
                    })
                }
            })
            .connect_lazy(&config.database_url)
            .map_err(PoolError::AsyncPoolCreationFailed)?;

        Ok(Pool {
            #[cfg(test)]
            pool: Arc::new(std::sync::Mutex::new(Some(pool))),
            #[cfg(not(test))]
            pool,
            async_pool,
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

    pub async fn get_async(&self) -> Result<AsyncPoolClient, PoolError> {
        match self.async_pool.acquire().await {
            Ok(conn) => Ok(conn),
            Err(err) => {
                self.metrics.failed_db_connections.inc();
                Err(PoolError::AsyncClientError(err))
            }
        }
    }

    pub(crate) fn used_connections(&self) -> u32 {
        self.with_pool(|p| p.state().connections - p.state().idle_connections)
            + (self.async_pool.size() - self.async_pool.num_idle() as u32)
    }

    pub(crate) fn idle_connections(&self) -> u32 {
        self.with_pool(|p| p.state().idle_connections) + self.async_pool.num_idle() as u32
    }

    pub(crate) fn max_size(&self) -> u32 {
        self.max_size
    }

    #[cfg(test)]
    pub(crate) fn shutdown(&self) {
        self.pool.lock().unwrap().take();
    }
}

/// This impl allows us to use our own pool as an executor for SQLx queries.
impl<'p> sqlx::Executor<'p> for &'_ Pool
where
    for<'c> &'c mut <sqlx::Postgres as sqlx::Database>::Connection:
        sqlx::Executor<'c, Database = sqlx::Postgres>,
{
    type Database = sqlx::Postgres;

    fn fetch_many<'e, 'q: 'e, E: 'q>(
        self,
        query: E,
    ) -> BoxStream<
        'e,
        Result<
            sqlx::Either<
                <sqlx::Postgres as sqlx::Database>::QueryResult,
                <sqlx::Postgres as sqlx::Database>::Row,
            >,
            sqlx::Error,
        >,
    >
    where
        E: sqlx::Execute<'q, Self::Database>,
    {
        self.async_pool.fetch_many(query)
    }

    fn fetch_optional<'e, 'q: 'e, E: 'q>(
        self,
        query: E,
    ) -> BoxFuture<'e, Result<Option<<sqlx::Postgres as sqlx::Database>::Row>, sqlx::Error>>
    where
        E: sqlx::Execute<'q, Self::Database>,
    {
        self.async_pool.fetch_optional(query)
    }

    fn prepare_with<'e, 'q: 'e>(
        self,
        sql: &'q str,
        parameters: &'e [<Self::Database as sqlx::Database>::TypeInfo],
    ) -> BoxFuture<
        'e,
        Result<<Self::Database as sqlx::database::HasStatement<'q>>::Statement, sqlx::Error>,
    > {
        self.async_pool.prepare_with(sql, parameters)
    }

    fn describe<'e, 'q: 'e>(
        self,
        sql: &'q str,
    ) -> BoxFuture<'e, Result<sqlx::Describe<Self::Database>, sqlx::Error>> {
        self.async_pool.describe(sql)
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

    #[error("failed to create the database connection pool")]
    AsyncPoolCreationFailed(#[source] sqlx::Error),

    #[error("failed to get a database connection")]
    ClientError(#[source] r2d2::Error),

    #[error("failed to get a database connection")]
    AsyncClientError(#[source] sqlx::Error),
}
