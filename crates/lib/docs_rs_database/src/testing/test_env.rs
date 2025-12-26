use crate::{AsyncPoolClient, Config, Pool, migrations};
use anyhow::{Context as _, Result};
use docs_rs_opentelemetry::AnyMeterProvider;
use futures_util::TryStreamExt as _;
use sqlx::Connection as _;
use tokio::{runtime, task::block_in_place};
use tracing::error;

#[derive(Debug)]
pub struct TestDatabase {
    pool: Pool,
    schema: String,
    runtime: runtime::Handle,
}

impl TestDatabase {
    pub async fn new(config: &Config, otel_meter_provider: &AnyMeterProvider) -> Result<Self> {
        // A random schema name is generated and used for the current connection. This allows each
        // test to create a fresh instance of the database to run within.
        //
        // TODO: potential performance improvements
        // * optionall use "DROP SCHEMA CASCADE" instead of rolling back migrations. But CI should
        //   still do it?
        // * use postgres template database? migrate once, just copy the template for each test?
        let schema = format!("docs_rs_test_schema_{}", rand::random::<u64>());

        let pool = Pool::new_with_schema(config, &schema, otel_meter_provider).await?;

        let mut conn = sqlx::PgConnection::connect(&config.database_url).await?;
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&mut conn)
            .await
            .context("error creating schema")?;
        sqlx::query(&format!("SET search_path TO {schema}, public"))
            .execute(&mut conn)
            .await
            .context("error setting search path")?;
        migrations::migrate(&mut conn, None)
            .await
            .context("error running migrations")?;

        // Move all sequence start positions 10000 apart to avoid overlapping primary keys
        let sequence_names: Vec<_> = sqlx::query!(
            "SELECT relname
             FROM pg_class
             INNER JOIN pg_namespace ON
                 pg_class.relnamespace = pg_namespace.oid
             WHERE pg_class.relkind = 'S'
                 AND pg_namespace.nspname = $1
            ",
            schema,
        )
        .fetch(&mut conn)
        .map_ok(|row| row.relname)
        .try_collect()
        .await?;

        for (i, sequence) in sequence_names.into_iter().enumerate() {
            let offset = (i + 1) * 10000;
            sqlx::query(&format!(
                r#"ALTER SEQUENCE "{sequence}" RESTART WITH {offset};"#
            ))
            .execute(&mut conn)
            .await?;
        }

        Ok(TestDatabase {
            pool,
            schema,
            runtime: runtime::Handle::current(),
        })
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    pub async fn async_conn(&self) -> Result<AsyncPoolClient> {
        self.pool.get_async().await.map_err(Into::into)
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let pool = self.pool.clone();
        let schema = self.schema.clone();
        let runtime = self.runtime.clone();

        block_in_place(move || {
            runtime.block_on(async move {
                let Ok(mut conn) = pool.get_async().await else {
                    error!("error in drop impl");
                    return;
                };

                let migration_result = migrations::migrate(&mut conn, Some(0)).await;

                if let Err(e) = sqlx::query(format!("DROP SCHEMA {} CASCADE;", schema).as_str())
                    .execute(&mut *conn)
                    .await
                {
                    error!("failed to drop test schema {}: {}", schema, e);
                    return;
                }

                if let Err(err) = migration_result {
                    error!(?err, "error reverting migrations");
                }
            })
        });
    }
}
