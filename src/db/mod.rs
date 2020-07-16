//! Database operations

pub(crate) use self::add_package::add_build_into_database;
pub(crate) use self::add_package::add_package_into_database;
pub use self::delete_crate::{delete_crate, delete_version};
pub use self::file::add_path_into_database;
pub use self::migrate::migrate;
pub use self::pool::{Pool, PoolError};

#[cfg(test)]
pub(crate) use self::pool::PoolConnection;

mod add_package;
pub mod blacklist;
mod delete_crate;
pub(crate) mod file;
mod migrate;
mod pool;
