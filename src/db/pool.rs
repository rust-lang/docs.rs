use postgres::Connection;
use std::env;
use std::marker::PhantomData;

#[cfg(test)]
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Debug, Clone)]
pub(crate) enum Pool {
    R2D2(r2d2::Pool<r2d2_postgres::PostgresConnectionManager>),
    #[cfg(test)]
    Simple(Arc<Mutex<Connection>>),
}

impl Pool {
    pub(crate) fn new() -> Pool {
        let db_url = env::var("CRATESFYI_DATABASE_URL")
            .expect("CRATESFYI_DATABASE_URL environment variable is not exists");

        let max_pool_size = env::var("DOCSRS_MAX_POOL_SIZE")
            .map(|s| {
                s.parse::<u32>()
                    .expect("DOCSRS_MAX_POOL_SIZE must be an integer")
            })
            .unwrap_or(90);
        crate::web::metrics::MAX_DB_CONNECTIONS.set(max_pool_size as i64);

        let min_pool_idle = env::var("DOCSRS_MIN_POOL_IDLE")
            .map(|s| {
                s.parse::<u32>()
                    .expect("DOCSRS_MIN_POOL_IDLE must be an integer")
            })
            .unwrap_or(10);

        let manager = r2d2_postgres::PostgresConnectionManager::new(
            &db_url[..],
            r2d2_postgres::TlsMode::None,
        )
        .expect("Failed to create PostgresConnectionManager");

        let pool = r2d2::Pool::builder()
            .max_size(max_pool_size)
            .min_idle(Some(min_pool_idle))
            .build(manager)
            .expect("Failed to create r2d2 pool");
        Pool::R2D2(pool)
    }

    #[cfg(test)]
    pub(crate) fn new_simple(conn: Arc<Mutex<Connection>>) -> Self {
        Pool::Simple(conn)
    }

    pub(crate) fn get<'a>(&'a self) -> Result<DerefConnection<'a>, PoolError> {
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

pub(crate) enum DerefConnection<'a> {
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
pub(crate) enum PoolError {
    #[fail(display = "failed to get a database connection")]
    ConnectionError(#[fail(cause)] r2d2::Error),
}
