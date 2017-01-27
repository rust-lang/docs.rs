//! Database operations

pub use self::add_package::add_package_into_database;
pub use self::add_package::add_build_into_database;
pub use self::file::add_path_into_database;

use postgres::{Connection, TlsMode};
use postgres::error::{Error, ConnectError};
use std::env;
use r2d2;
use r2d2_postgres;

mod add_package;
mod file;


/// Connects to database
pub fn connect_db() -> Result<Connection, ConnectError> {
    // FIXME: unwrap might not be the best here
    let db_url = env::var("CRATESFYI_DATABASE_URL")
        .expect("CRATESFYI_DATABASE_URL environment variable is not exists");
    Connection::connect(&db_url[..], TlsMode::None)
}


pub fn create_pool() -> r2d2::Pool<r2d2_postgres::PostgresConnectionManager> {
    let db_url = env::var("CRATESFYI_DATABASE_URL")
        .expect("CRATESFYI_DATABASE_URL environment variable is not exists");
    let config = r2d2::Config::default();
    let manager = r2d2_postgres::PostgresConnectionManager::new(&db_url[..],
                                                                r2d2_postgres::TlsMode::None)
        .expect("Failed to create PostgresConnectionManager");
    r2d2::Pool::new(config, manager).expect("Failed to create r2d2 pool")
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
    conn.execute("
        WITH doc as (
            SELECT DISTINCT ON(releases.crate_id)
                   releases.id,
                   releases.crate_id,
                   setweight(to_tsvector(crates.name), 'A')                                   ||
                   setweight(to_tsvector(coalesce(releases.description, '')), 'B')            ||
                   setweight(to_tsvector(coalesce((
                                SELECT string_agg(value, ' ')
                                FROM json_array_elements_text(releases.keywords)), '')), 'B') ||
                   setweight(to_tsvector(coalesce(releases.readme, '')), 'C')                 ||
                   setweight(to_tsvector(coalesce(releases.description_long, '')), 'C') as content
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
                 &[])
}


/// Creates database tables
pub fn create_tables(conn: &Connection) -> Result<(), Error> {
    let queries = ["CREATE TABLE crates ( \
            id SERIAL PRIMARY KEY, \
            name VARCHAR(255) UNIQUE NOT NULL, \
            latest_version_id INT DEFAULT 0, \
            versions JSON DEFAULT '[]', \
            downloads_total INT DEFAULT 0, \
            github_description VARCHAR(1024), \
            github_stars INT DEFAULT 0, \
            github_forks INT DEFAULT 0, \
            github_issues INT DEFAULT 0, \
            github_last_commit TIMESTAMP, \
            github_last_update TIMESTAMP, \
            content tsvector \
        )",
                   "CREATE TABLE releases ( \
            id SERIAL PRIMARY KEY, \
            crate_id INT NOT NULL REFERENCES crates(id), \
            version VARCHAR(100), \
            release_time TIMESTAMP, \
            dependencies JSON, \
            target_name VARCHAR(255), \
            yanked BOOL DEFAULT FALSE, \
            is_library BOOL DEFAULT TRUE, \
            build_status BOOL DEFAULT FALSE, \
            rustdoc_status BOOL DEFAULT FALSE, \
            test_status BOOL DEFAULT FALSE, \
            license VARCHAR(100), \
            repository_url VARCHAR(255), \
            homepage_url VARCHAR(255), \
            documentation_url VARCHAR(255), \
            description VARCHAR(1024), \
            description_long VARCHAR(51200), \
            readme VARCHAR(51200), \
            authors JSON, \
            keywords JSON, \
            have_examples BOOL DEFAULT FALSE, \
            downloads INT DEFAULT 0, \
            files JSON, \
            doc_targets JSON DEFAULT '[]', \
            doc_rustc_version VARCHAR(100) NOT NULL, \
            UNIQUE (crate_id, version) \
        )",
                   "CREATE TABLE authors ( \
            id SERIAL PRIMARY KEY, \
            name VARCHAR(255), \
            email VARCHAR(255), \
            slug VARCHAR(255) UNIQUE NOT NULL \
        )",
                   "CREATE TABLE author_rels ( \
            rid INT REFERENCES releases(id), \
            aid INT REFERENCES authors(id), \
            UNIQUE(rid, aid) \
        )",
                   "CREATE TABLE keywords ( \
            id SERIAL PRIMARY KEY, \
            name VARCHAR(255), \
            slug VARCHAR(255) NOT NULL UNIQUE \
        )",
                   "CREATE TABLE keyword_rels ( \
            rid INT REFERENCES releases(id), \
            kid INT REFERENCES keywords(id), \
            UNIQUE(rid, kid) \
        )",
                   "CREATE TABLE owners ( \
            id SERIAL PRIMARY KEY, \
            login VARCHAR(255) NOT NULL UNIQUE, \
            avatar VARCHAR(255), \
            name VARCHAR(255), \
            email VARCHAR(255) \
        )",
                   "CREATE TABLE owner_rels ( \
            cid INT REFERENCES releases(id), \
            oid INT REFERENCES owners(id), \
            UNIQUE(cid, oid) \
        )",
                   "CREATE TABLE builds ( \
            id SERIAL, \
            rid INT NOT NULL REFERENCES releases(id), \
            rustc_version VARCHAR(100) NOT NULL, \
            cratesfyi_version VARCHAR(100) NOT NULL, \
            build_status BOOL NOT NULL, \
            build_time TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
            output TEXT \
        )",
                   "CREATE TABLE queue ( \
            id SERIAL, \
            name VARCHAR(255), \
            version VARCHAR(100), \
            attempt INT DEFAULT 0, \
            date_added TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
            UNIQUE(name, version) \
        )",
                   "CREATE TABLE files ( \
            path VARCHAR(4096) NOT NULL PRIMARY KEY, \
            mime VARCHAR(100) NOT NULL, \
            date_added TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
            date_updated TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
            content BYTEA \
        )",
                   "CREATE INDEX ON releases (release_time DESC)",
                   "CREATE INDEX content_idx ON crates USING gin(content)",
                   "CREATE TABLE config ( \
            name VARCHAR(100) NOT NULL PRIMARY KEY, \
            value JSON NOT NULL \
        )",
                   "INSERT INTO config VALUES ('database_version', '1'::json)"];

    for query in queries.into_iter() {
        try!(conn.execute(query, &[]));
    }

    Ok(())
}



#[cfg(test)]
mod test {
    extern crate env_logger;
    use super::*;

    #[test]
    #[ignore]
    fn test_connect_db() {
        let conn = connect_db();
        assert!(conn.is_ok());
    }


    #[test]
    #[ignore]
    fn test_create_tables() {
        let _ = env_logger::init();
        let conn = connect_db();
        assert!(conn.is_ok());

        // FIXME: As expected this test always fails if database is already created
        let res = create_tables(&conn.unwrap());
        info!("RES: {:#?}", res);
        assert!(res.is_ok());
    }
}
