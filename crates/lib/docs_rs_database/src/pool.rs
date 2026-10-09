use crate::{Config, errors::PoolError, metrics::PoolMetrics};
use docs_rs_opentelemetry::AnyMeterProvider;
use futures_util::{TryStreamExt, future::BoxFuture, stream::BoxStream};
use sqlx::{AssertSqlSafe, Executor, SqlStr, postgres::PgPoolOptions};
use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
    time::Duration,
    time::Instant,
};
use tokio::runtime;
use tracing::debug;

const DEFAULT_SCHEMA: &str = "public";

#[derive(Debug, Clone)]
pub struct Pool {
    async_pool: sqlx::PgPool,
    runtime: runtime::Handle,
    otel_metrics: Arc<PoolMetrics>,
}

impl Pool {
    pub async fn new(
        config: &Config,
        otel_meter_provider: &AnyMeterProvider,
    ) -> Result<Pool, PoolError> {
        debug!(
            "creating database pool (if this hangs, consider running `docker-compose up -d db s3`)"
        );
        Self::new_inner(config, DEFAULT_SCHEMA, otel_meter_provider).await
    }

    #[cfg(any(test, feature = "testing"))]
    pub async fn new_with_schema(
        config: &Config,
        schema: &str,
        otel_meter_provider: &AnyMeterProvider,
    ) -> Result<Pool, PoolError> {
        Self::new_inner(config, schema, otel_meter_provider).await
    }

    async fn new_inner(
        config: &Config,
        schema: &str,
        otel_meter_provider: &AnyMeterProvider,
    ) -> Result<Pool, PoolError> {
        let acquire_timeout = Duration::from_secs(30);
        let max_lifetime = Duration::from_mins(30);
        let idle_timeout = Duration::from_mins(10);

        let mut options = PgPoolOptions::new()
            .max_connections(config.max_pool_size)
            .min_connections(config.min_pool_idle)
            .max_lifetime(max_lifetime)
            .acquire_timeout(acquire_timeout)
            .idle_timeout(idle_timeout);

        if cfg!(test) {
            options = options.test_before_acquire(false);
        }

        if schema != DEFAULT_SCHEMA {
            options = options.after_connect({
                let schema = schema.to_owned();
                move |conn, _meta| {
                    Box::pin({
                        let schema = schema.clone();

                        async move {
                            conn.execute(AssertSqlSafe(format!(
                                "SET search_path TO {schema}, {DEFAULT_SCHEMA};"
                            )))
                            .await?;

                            Ok(())
                        }
                    })
                }
            });
        }

        let async_pool = options
            .connect_lazy(config.database_url.as_str())
            .map_err(PoolError::AsyncPoolCreationFailed)?;

        Ok(Pool {
            async_pool: async_pool.clone(),
            runtime: runtime::Handle::current(),
            otel_metrics: Arc::new(PoolMetrics::new(async_pool, otel_meter_provider)),
        })
    }

    pub async fn get_async(&self) -> Result<AsyncPoolClient, PoolError> {
        self.acquire().await.map_err(PoolError::AsyncClientError)
    }

    /// Shared acquisition path for explicit clients and the Executor implementation.
    async fn acquire(&self) -> Result<AsyncPoolClient, sqlx::Error> {
        let guard = self.otel_metrics.start_acquire();
        match self.async_pool.acquire().await {
            Ok(conn) => {
                guard.finish("success");
                Ok(AsyncPoolClient {
                    inner: Some(conn),
                    runtime: self.runtime.clone(),
                    hold_duration: self.otel_metrics.connection_hold_duration.clone(),
                    acquired_at: Instant::now(),
                })
            }
            Err(err) => {
                guard.finish(if matches!(err, sqlx::Error::PoolTimedOut) {
                    "timeout"
                } else {
                    "error"
                });
                self.otel_metrics.failed_connections.add(1, &[]);
                Err(err)
            }
        }
    }
}

/// This impl allows us to use our own pool as an executor for SQLx queries.
impl<'c> sqlx::Executor<'c> for &'c Pool
where
    for<'conn> &'conn mut <sqlx::Postgres as sqlx::Database>::Connection:
        sqlx::Executor<'conn, Database = sqlx::Postgres>,
{
    type Database = sqlx::Postgres;

    fn fetch_many<'e, 'q: 'e, E>(
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
        'c: 'e,
        E: sqlx::Execute<'q, Self::Database> + 'q,
    {
        Box::pin(async_stream::try_stream! {
            let mut conn = self.acquire().await?;
            let mut stream = (&mut *conn).fetch_many(query);
            while let Some(item) = stream.try_next().await? {
                yield item;
            }
        })
    }

    fn fetch_optional<'e, 'q: 'e, E>(
        self,
        query: E,
    ) -> BoxFuture<'e, Result<Option<<sqlx::Postgres as sqlx::Database>::Row>, sqlx::Error>>
    where
        'c: 'e,
        E: sqlx::Execute<'q, Self::Database> + 'q,
    {
        Box::pin(async move {
            let mut conn = self.acquire().await?;
            (&mut *conn).fetch_optional(query).await
        })
    }

    fn prepare_with<'e>(
        self,
        sql: SqlStr,
        parameters: &'e [<Self::Database as sqlx::Database>::TypeInfo],
    ) -> BoxFuture<'e, Result<<Self::Database as sqlx::Database>::Statement, sqlx::Error>>
    where
        'c: 'e,
    {
        Box::pin(async move {
            let mut conn = self.acquire().await?;
            (&mut *conn).prepare_with(sql, parameters).await
        })
    }

    fn describe<'e>(
        self,
        sql: SqlStr,
    ) -> BoxFuture<'e, Result<sqlx::Describe<Self::Database>, sqlx::Error>>
    where
        'c: 'e,
    {
        Box::pin(async move {
            let mut conn = self.acquire().await?;
            (&mut *conn).describe(sql).await
        })
    }
}

/// we wrap `sqlx::PoolConnection` so we can drop it in a sync context
/// and enter the runtime.
/// Otherwise dropping the PoolConnection will panic because it can't spawn a task.
#[derive(Debug)]
pub struct AsyncPoolClient {
    inner: Option<sqlx::pool::PoolConnection<sqlx::postgres::Postgres>>,
    hold_duration: opentelemetry::metrics::Histogram<f64>,
    acquired_at: Instant,
    runtime: runtime::Handle,
}

impl Deref for AsyncPoolClient {
    type Target = sqlx::PgConnection;

    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().unwrap()
    }
}

impl DerefMut for AsyncPoolClient {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.as_mut().unwrap()
    }
}

impl Drop for AsyncPoolClient {
    fn drop(&mut self) {
        self.hold_duration
            .record(self.acquired_at.elapsed().as_secs_f64(), &[]);
        let _guard = self.runtime.enter();
        drop(self.inner.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use docs_rs_opentelemetry::testing::TestMetrics;

    #[tokio::test]
    async fn all_acquisition_paths_record_failures() {
        let telemetry = TestMetrics::new();
        let async_pool = sqlx::PgPool::connect_lazy("postgres://localhost/test").unwrap();
        async_pool.close().await;
        let pool = Pool {
            otel_metrics: Arc::new(PoolMetrics::new(async_pool.clone(), telemetry.provider())),
            async_pool,
            runtime: runtime::Handle::current(),
        };
        assert!(pool.get_async().await.is_err());
        assert!(
            (&pool)
                .fetch_optional(sqlx::query("SELECT 1"))
                .await
                .is_err()
        );
        let mut stream = (&pool).fetch_many(sqlx::query("SELECT 1"));
        assert!(stream.try_next().await.is_err());
        assert!(
            (&pool)
                .prepare_with(SqlStr::from_static("SELECT 1"), &[])
                .await
                .is_err()
        );
        assert!(
            (&pool)
                .describe(SqlStr::from_static("SELECT 1"))
                .await
                .is_err()
        );
        let collected = telemetry.collected_metrics();
        assert_eq!(
            collected
                .get_metric("pool", "docsrs.db.pool.acquires")
                .unwrap()
                .get_u64_counter()
                .value(),
            5
        );
        assert_eq!(
            collected
                .get_metric("pool", "docsrs.db.pool.failed_connections")
                .unwrap()
                .get_u64_counter()
                .value(),
            5
        );
        assert_eq!(
            collected
                .get_metric("pool", "docsrs.db.pool.acquire_duration")
                .unwrap()
                .get_f64_histogram()
                .count(),
            5
        );
    }
    #[tokio::test]
    async fn hold_is_recorded_only_when_released() {
        let telemetry = TestMetrics::new();
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test").unwrap();
        let metrics = PoolMetrics::new(pool, telemetry.provider());
        let client = AsyncPoolClient {
            inner: None,
            hold_duration: metrics.connection_hold_duration.clone(),
            acquired_at: Instant::now(),
            runtime: runtime::Handle::current(),
        };
        assert!(
            telemetry
                .collected_metrics()
                .get_metric("pool", "docsrs.db.pool.connection_hold_duration")
                .is_err()
        );
        drop(client);
        assert_eq!(
            telemetry
                .collected_metrics()
                .get_metric("pool", "docsrs.db.pool.connection_hold_duration")
                .unwrap()
                .get_f64_histogram()
                .count(),
            1
        );
    }
}
