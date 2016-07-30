//! Database operations

pub use self::add_package::add_package_into_database;
pub use self::add_package::add_build_into_database;
pub use self::file::add_path_into_database;

use postgres::{Connection, SslMode};
use postgres::error::{Error, ConnectError};
use std::env;

mod add_package;
mod file;


/// Connects to database
pub fn connect_db() -> Result<Connection, ConnectError> {
    // FIXME: unwrap might not be the best here
    let db_url = env::var("CRATESFYI_DATABASE_URL")
        .expect("CRATESFYI_DATABASE_URL environment variable is not exists");
    Connection::connect(&db_url[..], SslMode::None)
}


/// Creates database tables
pub fn create_tables(conn: &Connection) -> Result<(), Error> {
    let queries = [
        "CREATE TABLE crates ( \
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
            github_last_update TIMESTAMP \
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
            UNIQUE(name, version) \
        )",
        "CREATE TABLE files ( \
            path VARCHAR(4096) NOT NULL PRIMARY KEY, \
            mime VARCHAR(100) NOT NULL, \
            date_added TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
            date_updated TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
            content BYTEA \
        )",
        "CREATE TABLE config ( \
            name VARCHAR(100) NOT NULL PRIMARY KEY, \
            value JSON NOT NULL \
        )",
        "INSERT INTO config VALUES ('database_version', '1'::json)",
    ];

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
