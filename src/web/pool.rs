use db::create_pool;
use iron::{typemap, BeforeMiddleware, IronResult, Request};
use postgres::Connection;
use r2d2;
use r2d2_postgres;

pub(crate) enum Pool {
    R2D2(r2d2::Pool<r2d2_postgres::PostgresConnectionManager>),
}

impl Pool {
    pub(crate) fn new() -> Pool {
        Pool::R2D2(create_pool())
    }
}

impl typemap::Key for Pool {
    type Value = PoolConnection;
}

impl BeforeMiddleware for Pool {
    fn before(&self, req: &mut Request) -> IronResult<()> {
        req.extensions.insert::<Pool>(match self {
            Self::R2D2(pool) => PoolConnection::R2D2(pool.get().unwrap()),
        });
        Ok(())
    }
}

pub(crate) enum PoolConnection {
    R2D2(r2d2::PooledConnection<r2d2_postgres::PostgresConnectionManager>),
}

impl PoolConnection {
    pub(super) fn get<'a>(&'a self) -> DerefConnection<'a> {
        match self {
            Self::R2D2(conn) => DerefConnection::Connection(&conn),
        }
    }
}

pub(crate) enum DerefConnection<'a> {
    Connection(&'a Connection),
}

impl<'a> std::ops::Deref for DerefConnection<'a> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        match self {
            Self::Connection(conn) => conn,
        }
    }
}
