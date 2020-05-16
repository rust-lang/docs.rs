use crate::db::create_pool;
use iron::{status::Status, typemap, BeforeMiddleware, IronError, IronResult, Request};
use postgres::Connection;
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
        Pool::R2D2(create_pool())
    }

    #[cfg(test)]
    pub(crate) fn new_simple(conn: Arc<Mutex<Connection>>) -> Self {
        Pool::Simple(conn)
    }

    pub(super) fn get<'a>(&'a self) -> IronResult<DerefConnection<'a>> {
        match self {
            Self::R2D2(conn) => {
                let conn = conn.get().map_err(|err| {
                    log::error!("Error getting db connection: {:?}", err);
                    super::metrics::FAILED_DB_CONNECTIONS.inc();

                    IronError::new(err, Status::InternalServerError)
                })?;

                Ok(DerefConnection::Connection(conn, PhantomData))
            }

            #[cfg(test)]
            Self::Simple(mutex) => Ok(DerefConnection::Guard(
                mutex.lock().expect("failed to lock the connection"),
            )),
        }
    }
}

impl typemap::Key for Pool {
    type Value = Pool;
}

impl BeforeMiddleware for Pool {
    fn before(&self, req: &mut Request) -> IronResult<()> {
        req.extensions.insert::<Pool>(self.clone());

        Ok(())
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
