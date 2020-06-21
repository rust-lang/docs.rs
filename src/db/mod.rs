//! Database operations

pub(crate) use self::add_package::add_build_into_database;
pub(crate) use self::add_package::add_package_into_database;
pub use self::delete_crate::delete_crate;
pub use self::file::add_path_into_database;
pub use self::migrate::migrate;
pub(crate) use self::pool::{Pool, PoolError};

use failure::Fail;
use postgres::{Connection, TlsMode};
use std::env;

mod add_package;
pub mod blacklist;
mod delete_crate;
pub(crate) mod file;
mod migrate;
mod pool;

/// Connects to database
pub fn connect_db() -> Result<Connection, failure::Error> {
    let err = "CRATESFYI_DATABASE_URL environment variable is not set";
    let db_url = env::var("CRATESFYI_DATABASE_URL").map_err(|e| e.context(err))?;
    Connection::connect(&db_url[..], TlsMode::None).map_err(Into::into)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    #[ignore]
    fn test_connect_db() {
        let conn = connect_db();
        assert!(conn.is_ok());
    }
}
