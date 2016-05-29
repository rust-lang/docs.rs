//! Web interface of cratesfyi

pub use self::rustdoc::start_rustdoc_web_server;

mod rustdoc;

use ::db;
use iron::prelude::*;
use iron::{BeforeMiddleware, typemap};
use postgres;


/// Simple iron middleware for database connection
struct DbConnection;

impl typemap::Key for DbConnection { type Value = postgres::Connection; }

impl BeforeMiddleware for DbConnection {
    fn before(&self, req: &mut Request) -> IronResult<()> {
        req.extensions.insert::<DbConnection>(db::connect_db().unwrap());
        Ok(())
    }
}
