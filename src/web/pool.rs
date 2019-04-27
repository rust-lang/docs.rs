

use iron::prelude::*;
use iron::{BeforeMiddleware, typemap};
use crate::db::create_pool;


pub struct Pool {
    pool: r2d2::Pool<r2d2_postgres::PostgresConnectionManager>,
}

impl typemap::Key for Pool {
    type Value = r2d2::PooledConnection<r2d2_postgres::PostgresConnectionManager>;
}

impl Pool {
    pub fn new() -> Pool {
        Pool { pool: create_pool() }
    }
}

impl BeforeMiddleware for Pool {
    fn before(&self, req: &mut Request<'_, '_>) -> IronResult<()> {
        req.extensions.insert::<Pool>(self.pool.get().unwrap());
        Ok(())
    }
}
