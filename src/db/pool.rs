use crate::Config;
use postgres::Connection;
use std::marker::PhantomData;

#[cfg(test)]
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Debug, Clone)]
pub enum Pool {
    R2D2(r2d2::Pool<r2d2_postgres::PostgresConnectionManager>),
    #[cfg(test)]
    Simple(Arc<Mutex<Connection>>),
}

impl Pool {
    pub fn new(config: &Config) -> Result<Pool, PoolError> {
        crate::web::metrics::MAX_DB_CONNECTIONS.set(config.max_pool_size as i64);

        let manager = r2d2_postgres::PostgresConnectionManager::new(
            config.database_url.as_str(),
            r2d2_postgres::TlsMode::None,
        )
        .map_err(PoolError::InvalidDatabaseUrl)?;

        let pool = r2d2::Pool::builder()
            .max_size(config.max_pool_size)
            .min_idle(Some(config.min_pool_idle))
            .build(manager)
            .map_err(PoolError::PoolCreationFailed)?;

        Ok(Pool::R2D2(pool))
    }

    #[cfg(test)]
    pub(crate) fn new_simple(conn: Arc<Mutex<Connection>>) -> Self {
        Pool::Simple(conn)
    }

    pub fn get(&self) -> Result<DerefConnection<'_>, PoolError> {
        match self {
            Self::R2D2(r2d2) => match r2d2.get() {
                Ok(conn) => Ok(DerefConnection::Connection(conn, PhantomData)),
                Err(err) => {
                    crate::web::metrics::FAILED_DB_CONNECTIONS.inc();
                    Err(PoolError::ConnectionError(err))
                }
            },

            #[cfg(test)]
            Self::Simple(mutex) => Ok(DerefConnection::Guard(
                mutex.lock().expect("failed to lock the connection"),
            )),
        }
    }

    pub(crate) fn used_connections(&self) -> u32 {
        match self {
            Self::R2D2(conn) => conn.state().connections - conn.state().idle_connections,

            #[cfg(test)]
            Self::Simple(..) => 0,
        }
    }

    pub(crate) fn idle_connections(&self) -> u32 {
        match self {
            Self::R2D2(conn) => conn.state().idle_connections,

            #[cfg(test)]
            Self::Simple(..) => 0,
        }
    }
}

pub enum DerefConnection<'a> {
    Connection(
        r2d2::PooledConnection<r2d2_postgres::PostgresConnectionManager>,
        PhantomData<&'a ()>,
    ),

    #[cfg(test)]
    Guard(MutexGuard<'a, Connection>),
}

impl<'a> std::ops::Deref for DerefConnection<'a> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        match self {
            Self::Connection(conn, ..) => conn,

            #[cfg(test)]
            Self::Guard(guard) => &guard,
        }
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
