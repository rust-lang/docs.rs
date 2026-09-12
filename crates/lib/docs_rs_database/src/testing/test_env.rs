use crate::{AsyncPoolClient, Config, Pool, migrations};
use anyhow::{Context as _, Result, bail};
use docs_rs_opentelemetry::AnyMeterProvider;
use docs_rs_utils::spawn_blocking;
use rand::{RngExt as _, distr::Alphanumeric};
use sqlx::{AssertSqlSafe, Connection as _};
use std::{env, iter, path::PathBuf};
use tempfile::NamedTempFile;
use tokio::{
    fs, io::AsyncWriteExt as _, process::Command, runtime, sync::OnceCell, task::block_in_place,
};
use tracing::{error, instrument, warn};

const TEST_SCHEMA_PREFIX: &str = "docs_rs_test_schema_";
const TEMPLATE_SCHEMA: &str = "docs_rs_test_template";
pub const TEMPLATE_DDL_ENV: &str = "DOCSRS_TEST_DATABASE_DDL_PATH";

static TEMPLATE_DDL: OnceCell<String> = OnceCell::const_new();

/// An isolated test schema cloned from a fresh, fully migrated template.
///
/// The template is prepared once, then each test replays its schema-only DDL
/// into a fresh schema that is dropped when this value is dropped.
///
#[derive(Debug)]
pub struct TestDatabase {
    pool: Pool,
    schema: String,
    runtime: runtime::Handle,
}

impl TestDatabase {
    #[instrument(skip_all)]
    pub async fn new(config: &Config, otel_meter_provider: &AnyMeterProvider) -> Result<Self> {
        let template_ddl = get_template_schema_ddl(config).await?;
        let schema = format!("{TEST_SCHEMA_PREFIX}{}", generate_name());

        let mut conn = sqlx::PgConnection::connect(config.database_url.as_str()).await?;

        // run the prepared DDL to fill the database schema into the new schema.
        //
        // The DDL is produced by pg_dump from a schema we own. The only substitution is a
        // generated schema name, so it is safe to send as raw SQL.
        sqlx::raw_sql(AssertSqlSafe(
            template_ddl.replace(TEMPLATE_SCHEMA, &schema),
        ))
        .execute(&mut conn)
        .await
        .context("error cloning test database schema")?;

        let pool = Pool::new_with_schema(config, &schema, otel_meter_provider).await?;

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

                // NOTE: we run all reverse-migrations after the tests.
                // With that we ensure that even with data, the rollback will work.
                // This only costs little performance at the moment, we could make
                // this optional at some point.
                let migration_error = migrations::migrate(&mut conn, Some(0)).await.err();

                if let Err(e) = sqlx::query(AssertSqlSafe(format!(
                    "DROP SCHEMA IF EXISTS {schema} CASCADE;"
                )))
                .execute(&mut *conn)
                .await
                {
                    panic!("failed to drop test schema {schema}: {e}");
                }

                if let Some(err) = migration_error {
                    panic!("failed to revert migrations for test schema {schema}: {err:?}");
                }
            })
        });
    }
}

/// Rebuilds the migrated template schema, dumps its DDL, and keeps that dump
/// in a persistent temporary file. The nextest setup script exposes this path
/// to every test process, avoiding one migration run per process.
#[instrument(skip_all)]
pub async fn prepare_template_schema(config: &Config) -> Result<PathBuf> {
    let template_ddl = create_template_schema_and_ddl(config).await?;

    let (mut file, path) = spawn_blocking(|| {
        let (file, path) = NamedTempFile::new()?.keep()?;

        Ok((fs::File::from_std(file), path))
    })
    .await
    .context("error creating temporary file")?;

    file.write_all(template_ddl.as_bytes())
        .await
        .context("error writing template DDL file")?;
    file.flush().await?;

    Ok(path)
}

#[instrument(skip_all)]
async fn get_template_schema_ddl(config: &Config) -> Result<&'static String> {
    TEMPLATE_DDL
        .get_or_try_init(|| async {
            if let Some(path) = env::var_os(TEMPLATE_DDL_ENV) {
                fs::read_to_string(path).await.context("error reading template DDL file")
            } else {
                warn!("fall back to generating template DDL ourselves, cargo nexttest setup script wan't run");
                create_template_schema_and_ddl(config).await.context("error generating new template schema DDL")
            }
        })
        .await
}

#[instrument(skip_all)]
async fn create_template_schema_and_ddl(config: &Config) -> Result<String> {
    let mut conn = sqlx::PgConnection::connect(config.database_url.as_str()).await?;

    // Cargo test can start several test binaries at once. Serializing this work keeps them from
    // racing while rebuilding the shared template schema.
    sqlx::query("SELECT pg_advisory_lock(hashtext($1))")
        .bind(TEMPLATE_SCHEMA)
        .execute(&mut conn)
        .await?;

    let result = async {
        sqlx::query(AssertSqlSafe(format!(
            "DROP SCHEMA IF EXISTS {TEMPLATE_SCHEMA} CASCADE"
        )))
        .execute(&mut conn)
        .await?;

        sqlx::query(AssertSqlSafe(format!("CREATE SCHEMA {TEMPLATE_SCHEMA}")))
            .execute(&mut conn)
            .await?;

        sqlx::query(AssertSqlSafe(format!(
            "SET search_path TO {TEMPLATE_SCHEMA}, public"
        )))
        .execute(&mut conn)
        .await?;

        // The newly-created schema has no migration history, so exercise every forward migration.
        migrations::migrate(&mut conn, None).await?;

        dump_schema(config).await
    }
    .await;

    sqlx::query("SELECT pg_advisory_unlock(hashtext($1))")
        .bind(TEMPLATE_SCHEMA)
        .execute(&mut conn)
        .await?;

    result
}

/// Captures schema DDL suitable for sending directly to PostgreSQL through SQLx.
#[instrument(skip_all)]
async fn dump_schema(config: &Config) -> Result<String> {
    let args = [
        // --schema-only: emit DDL only—no table data.
        "--schema-only",
        // --no-owner: omit ownership-changing statements.
        "--no-owner",
        // --no-acl: omit grants/revokes.
        "--no-acl",
        // --schema docs_rs_test_template: dump only the template schema.
        "--schema",
        TEMPLATE_SCHEMA,
    ];

    // Sometimes the `pg_dump` (`postgresql-client`) version on the developer or CI
    // host is too old. In this case, you can fall back to using
    // `docker compose exec` to generate the template DDL. This is not the default,
    // because that would bind the test execution to `docker compose` by default and
    // is slower than using `pg_dump` directly.
    let mut command = if config.use_pg_dump_from_docker_compose {
        let username = config.database_url.username();

        if username.is_empty() {
            bail!("database URL must include a username when dumping through Docker Compose");
        }

        let database = config.database_url.path().trim_start_matches('/');

        let mut command = Command::new("docker");
        // some args explained:
        // * `-T`: disable pseudo-TTY allocation. This makes stdout a clean pipe so Rust can
        //   capture the SQL dump reliably; otherwise Compose may add terminal behavior
        // * `db`: the Compose service containing PostgreSQL 18 and its matching pg_dump.
        //   Must match our `docker-compose.yaml`.

        command
            .args(["compose", "exec", "-T", "db", "pg_dump"])
            .args(args)
            .args(["--username", username]);

        if !database.is_empty() {
            command.args(["--dbname", database]);
        }

        command
    } else {
        let mut command = Command::new("pg_dump");
        command.args(args).arg(config.database_url.as_str());
        command
    };

    let output = command
        .output()
        .await
        .context("error running pg_dump for test template")?;

    if !output.status.success() {
        bail!(
            "pg_dump for test template failed: \n{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let ddl = String::from_utf8(output.stdout).context("pg_dump output was not UTF-8")?;
    // pg_dump's plain output targets psql. SQLx sends it straight to PostgreSQL,
    // so remove psql meta-commands and settings unsupported by older servers.
    Ok(ddl
        .lines()
        .filter(|line| !line.starts_with('\\')) // meta commands
        .filter(|line| !line.trim_start().starts_with("SET transaction_timeout"))
        .collect::<Vec<_>>()
        .join("\n"))
}

fn generate_name() -> String {
    let mut rng = rand::rng();
    iter::repeat(())
        .map(|_| rng.sample(Alphanumeric) as char)
        .take(16)
        .collect::<String>()
        .to_lowercase()
}
