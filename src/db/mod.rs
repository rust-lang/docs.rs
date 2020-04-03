//! Database operations

pub(crate) use self::add_package::add_package_into_database;
pub(crate) use self::add_package::add_build_into_database;
pub(crate) use self::add_package::CratesIoData;
pub use self::file::{add_path_into_database, move_to_s3};
pub use self::migrate::migrate;
#[cfg(test)]
pub(crate) use self::migrate::migrate_temporary;
pub use self::delete_crate::delete_crate;

use postgres::{Connection, TlsMode};
use postgres::error::Error;
use std::env;
use r2d2;
use r2d2_postgres;

mod add_package;
pub mod blacklist;
mod delete_crate;
pub(crate) mod file;
mod migrate;

/// Connects to database
pub fn connect_db() -> Result<Connection, Error> {
    // FIXME: unwrap might not be the best here
    let db_url = env::var("CRATESFYI_DATABASE_URL")
        .expect("CRATESFYI_DATABASE_URL environment variable is not exists");
    Connection::connect(&db_url[..], TlsMode::None)
}

pub(crate) fn create_pool() -> r2d2::Pool<r2d2_postgres::PostgresConnectionManager> {
    let db_url = env::var("CRATESFYI_DATABASE_URL")
        .expect("CRATESFYI_DATABASE_URL environment variable is not exists");
    let manager =
        r2d2_postgres::PostgresConnectionManager::new(&db_url[..], r2d2_postgres::TlsMode::None)
            .expect("Failed to create PostgresConnectionManager");
    r2d2::Pool::builder()
        .build(manager)
        .expect("Failed to create r2d2 pool")
}

/// Updates content column in crates table.
///
/// This column will be used for searches and always contains `tsvector` of:
///
///   * crate name (rank A-weight)
///   * latest release description (rank B-weight)
///   * latest release keywords (rank B-weight)
///   * latest release readme (rank C-weight)
///   * latest release root rustdoc (rank C-weight)
pub fn update_search_index(conn: &Connection) -> Result<u64, Error> {
    conn.execute(
        "
        WITH doc as (
            SELECT DISTINCT ON(releases.crate_id)
                   releases.id,
                   releases.crate_id,
                   setweight(to_tsvector(crates.name), 'A')                                   ||
                   setweight(to_tsvector(coalesce(releases.description, '')), 'B')            ||
                   setweight(to_tsvector(coalesce((
                                SELECT string_agg(value, ' ')
                                FROM json_array_elements_text(releases.keywords)), '')), 'B')
                    as content
            FROM releases
            INNER JOIN crates ON crates.id = releases.crate_id
            ORDER BY releases.crate_id, releases.release_time DESC
        )
        UPDATE crates
        SET latest_version_id = doc.id,
            content = doc.content
        FROM doc
        WHERE crates.id = doc.crate_id AND
            (crates.latest_version_id = 0 OR crates.latest_version_id != doc.id);",
        &[],
    )
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
