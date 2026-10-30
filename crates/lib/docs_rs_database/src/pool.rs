use crate::{Config, errors::PoolError, metrics::PoolMetrics};
use docs_rs_opentelemetry::AnyMeterProvider;
use futures_util::{TryStreamExt as _, future::BoxFuture, stream::BoxStream};
use sqlx::{AssertSqlSafe, Executor, SqlStr, pool::PoolConnection, postgres::PgPoolOptions};
use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::runtime;
use tracing::{debug, instrument};

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
        Ok(AsyncPoolClient {
            inner: Some(self.acquire().await.map_err(PoolError::AsyncClientError)?),
            runtime: self.runtime.clone(),
        })
    }

    #[instrument(skip_all)]
    async fn acquire(&self) -> Result<PoolConnection<sqlx::Postgres>, sqlx::Error> {
        let start = Instant::now();
        let result = self.async_pool.acquire().await;
        self.otel_metrics
            .acquire_duration
            .record(start.elapsed().as_secs_f64(), &[]);
        match result {
            Ok(conn) => Ok(conn),
            Err(err) => {
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
        Box::pin(async move { (&mut *self.acquire().await?).fetch_optional(query).await })
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
            (&mut *self.acquire().await?)
                .prepare_with(sql, parameters)
                .await
        })
    }

    fn describe<'e>(
        self,
        sql: SqlStr,
    ) -> BoxFuture<'e, Result<sqlx::Describe<Self::Database>, sqlx::Error>>
    where
        'c: 'e,
    {
        Box::pin(async move { (&mut *self.acquire().await?).describe(sql).await })
    }
}

/// we wrap `sqlx::PoolConnection` so we can drop it in a sync context
/// and enter the runtime.
/// Otherwise dropping the PoolConnection will panic because it can't spawn a task.
#[derive(Debug)]
pub struct AsyncPoolClient {
    inner: Option<PoolConnection<sqlx::Postgres>>,
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
        let _guard = self.runtime.enter();
        drop(self.inner.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use docs_rs_config::AppConfig as _;
    use docs_rs_opentelemetry::testing::TestMetrics;
    use sqlx::SqlSafeStr as _;

    #[tokio::test]
    async fn acquire_duration_includes_success_and_failure() -> anyhow::Result<()> {
        let metrics = TestMetrics::new();
        let pool = Pool::new(&Config::test_config()?, metrics.provider()).await?;

        drop(pool.get_async().await?);
        pool.async_pool.close().await;
        assert!(pool.get_async().await.is_err());

        let collected = metrics.collected_metrics();
        let metric = collected.get_metric("pool", "docsrs.db.pool.acquire_duration")?;
        assert_eq!(metric.unit(), "s");
        let histogram = metric.get_f64_histogram();
        assert_eq!(histogram.count(), 2);
        assert!(histogram.sum() > 0.0);
        Ok(())
    }

    #[tokio::test]
    async fn executor_records_acquisitions_and_preserves_errors() -> anyhow::Result<()> {
        let metrics = TestMetrics::new();
        let pool = Pool::new(&Config::test_config()?, metrics.provider()).await?;

        assert_eq!(sqlx::query("SELECT 1").fetch_all(&pool).await?.len(), 1);
        assert!(
            sqlx::query("SELECT 1")
                .fetch_optional(&pool)
                .await?
                .is_some()
        );
        pool.prepare_with("SELECT 1".into_sql_str(), &[]).await?;
        pool.describe("SELECT 1".into_sql_str()).await?;

        pool.async_pool.close().await;
        assert!(matches!(
            sqlx::query("SELECT 1").fetch_all(&pool).await,
            Err(sqlx::Error::PoolClosed)
        ));
        assert!(matches!(
            sqlx::query("SELECT 1").fetch_optional(&pool).await,
            Err(sqlx::Error::PoolClosed)
        ));
        assert!(matches!(
            pool.prepare_with("SELECT 1".into_sql_str(), &[]).await,
            Err(sqlx::Error::PoolClosed)
        ));
        assert!(matches!(
            pool.describe("SELECT 1".into_sql_str()).await,
            Err(sqlx::Error::PoolClosed)
        ));

        let collected = metrics.collected_metrics();
        assert_eq!(
            collected
                .get_metric("pool", "docsrs.db.pool.acquire_duration")?
                .get_f64_histogram()
                .count(),
            8
        );
        assert_eq!(
            collected
                .get_metric("pool", "docsrs.db.pool.failed_connections")?
                .get_u64_counter()
                .value(),
            4
        );
        Ok(())
    }

    #[tokio::test]
    async fn executor_stream_is_lazy_and_releases_connection_on_drop() -> anyhow::Result<()> {
        let metrics = TestMetrics::new();
        let mut config = Config::test_config()?;
        config.max_pool_size = 1;
        config.min_pool_idle = 0;
        let pool = Pool::new(&config, metrics.provider()).await?;
        let mut stream = sqlx::query("SELECT generate_series(1, 10)").fetch(&pool);

        // Merely constructing the stream must not acquire a connection.
        let conn = pool.get_async().await?;
        drop(conn);
        assert!(stream.try_next().await?.is_some());
        drop(stream);

        // Dropping a partially consumed stream returns its connection to the pool.
        let conn = tokio::time::timeout(Duration::from_secs(2), pool.get_async()).await??;
        drop(conn);
        Ok(())
    }
}
