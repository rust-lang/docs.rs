use anyhow::Result;
use moka::sync::Cache;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, OpenFlags};
use std::{
    num::NonZeroU64,
    path::{Path, PathBuf},
    time::Duration,
};

static MAX_IDLE_TIME: Duration = Duration::from_secs(10 * 60);
static MAX_LIFE_TIME: Duration = Duration::from_secs(60 * 60);

/// SQLite connection pool.
///
/// Typical connection pools handle many connections to a single database,
/// while this one handles some connections to many databases.
///
/// The more connections we keep alive, the more open files we have,
/// so you might need to tweak this limit based on the max open files
/// on your system.
///
/// We open the databases in readonly mode.
/// We are using an additional connection pool per database to parallel requests
/// can be efficiently answered. Because of this the actual max connection count
/// might be higher than the given max_connections.
///
/// We keep at minimum of one connection per database, for one hour.  
/// Any additional connections will be dropped after 10 minutes of inactivity.
#[derive(Clone)]
pub(crate) struct SqliteConnectionPool {
    pools: Cache<PathBuf, r2d2::Pool<SqliteConnectionManager>>,
}

impl Default for SqliteConnectionPool {
    fn default() -> Self {
        Self::new(NonZeroU64::new(10).unwrap())
    }
}

impl SqliteConnectionPool {
    pub(crate) fn new(max_connections: NonZeroU64) -> Self {
        Self {
            pools: Cache::builder()
                .max_capacity(max_connections.get())
                .time_to_idle(MAX_LIFE_TIME)
                .build(),
        }
    }

    pub(crate) fn with_connection<R, P: AsRef<Path>, F: Fn(&Connection) -> Result<R>>(
        &self,
        path: P,
        f: F,
    ) -> Result<R> {
        let path = path.as_ref().to_owned();

        let pool = self
            .pools
            .entry(path.clone())
            .or_insert_with(|| {
                let manager = SqliteConnectionManager::file(path)
                    .with_flags(OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX);
                r2d2::Pool::builder()
                    .min_idle(Some(1))
                    .max_lifetime(Some(MAX_LIFE_TIME))
                    .idle_timeout(Some(MAX_IDLE_TIME))
                    .max_size(10)
                    .build_unchecked(manager)
            })
            .into_value();

        let conn = pool.get()?;
        f(&conn)
    }
}
