use crate::Config;
use postgres::Connection;
use r2d2_postgres::PostgresConnectionManager;

pub(crate) type PoolConnection = r2d2::PooledConnection<PostgresConnectionManager>;

const DEFAULT_SCHEMA: &str = "public";

#[derive(Debug, Clone)]
pub struct Pool {
    pool: r2d2::Pool<PostgresConnectionManager>,
}

impl Pool {
    pub fn new(config: &Config) -> Result<Pool, PoolError> {
        Self::new_inner(config, DEFAULT_SCHEMA)
    }

    #[cfg(test)]
    pub(crate) fn new_with_schema(config: &Config, schema: &str) -> Result<Pool, PoolError> {
        Self::new_inner(config, schema)
    }

    fn new_inner(config: &Config, schema: &str) -> Result<Pool, PoolError> {
        crate::web::metrics::MAX_DB_CONNECTIONS.set(config.max_pool_size as i64);

        let manager = PostgresConnectionManager::new(
            config.database_url.as_str(),
            r2d2_postgres::TlsMode::None,
        )
        .map_err(PoolError::InvalidDatabaseUrl)?;

        let pool = r2d2::Pool::builder()
            .max_size(config.max_pool_size)
            .min_idle(Some(config.min_pool_idle))
            .connection_customizer(Box::new(SetSchema::new(schema)))
            .build(manager)
            .map_err(PoolError::PoolCreationFailed)?;

        Ok(Pool { pool })
    }

    pub fn get(&self) -> Result<PoolConnection, PoolError> {
        match self.pool.get() {
            Ok(conn) => Ok(conn),
            Err(err) => {
                crate::web::metrics::FAILED_DB_CONNECTIONS.inc();
                Err(PoolError::ConnectionError(err))
            }
        }
    }

    pub(crate) fn used_connections(&self) -> u32 {
        self.pool.state().connections - self.pool.state().idle_connections
    }

    pub(crate) fn idle_connections(&self) -> u32 {
        self.pool.state().idle_connections
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

impl r2d2::CustomizeConnection<Connection, postgres::Error> for SetSchema {
    fn on_acquire(&self, conn: &mut Connection) -> Result<(), postgres::Error> {
        if self.schema != DEFAULT_SCHEMA {
            conn.execute(
                &format!("SET search_path TO {}, {};", self.schema, DEFAULT_SCHEMA),
                &[],
            )?;
        }
        Ok(())
    }
}

#[derive(Debug, failure::Fail)]
pub enum PoolError {
    #[fail(display = "the provided database URL was not valid")]
    InvalidDatabaseUrl(#[fail(cause)] postgres::Error),

    #[fail(display = "failed to create the connection pool")]
    PoolCreationFailed(#[fail(cause)] r2d2::Error),

    #[fail(display = "failed to get a database connection")]
    ConnectionError(#[fail(cause)] r2d2::Error),
}
