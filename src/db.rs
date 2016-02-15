//! Database operations

use postgres::{Connection, SslMode};
use postgres::error::ConnectError;


const DB_CONNECTION_STR: &'static str = "postgresql://cratesfyi@localhost";


/// Connects to database
pub fn connect_db() -> Result<Connection, ConnectError> {
    Connection::connect(DB_CONNECTION_STR, SslMode::None)
}


/// Creates database tables
pub fn create_tables(conn: &Connection) {
    let tables = [
        "CREATE TABLE crates ( \
            id SERIAL, \
            name text UNIQUE NOT NULL, \
            latest_version_id INT DEFAULT 0, \
            github_stars INT DEFAULT 0, \
            badges JSON, \
            issues JSON, \
            downloads_total INT DEFAULT 0 \
        )",
        "CREATE TABLE releases ( \
            id SERIAL, \
            crate_id INT NOT NULL, \
            version TEXT, \
            release_time TIMESTAMP, \
            dependencies JSON, \
            yanked BOOL DEFAULT FALSE, \
            build_status INT DEFAULT 0, \
            rustdoc_status INT DEFAULT 0, \
            test_status INT DEFAULT 0, \
            license TEXT, \
            repository_url TEXT, \
            homepage_url TEXT, \
            description TEXT, \
            description_long TEXT, \
            readme TEXT, \
            authors JSON, \
            keywords JSON, \
            downloads INT DEFAULT 0, \
            UNIQUE (crate_id, version) \
        )",
        "CREATE TABLE authors ( \
            id SERIAL, \
            name TEXT NOT NULL, \
            email TEXT, \
            slug TEXT UNIQUE NOT NULL, \
            github_url TEXT, \
            github_profile_picture_url TEXT \
        )",
        "CREATE TABLE author_rels ( \
            cid INT, \
            aid INT, \
            UNIQUE(cid, aid) \
        )",
        "CREATE TABLE keywords ( \
            id SERIAL, \
            name TEXT, \
            slug TEXT NOT NULL UNIQUE \
        )",
        "CREATE TABLE keyword_rels ( \
            cid INT, \
            kid INT, \
            UNIQUE(cid, kid) \
        )"
    ];

    for table in tables.into_iter() {
        if let Err(e) = conn.execute(table, &[]) {
            println!("{}", e);
        }
    }
}



#[test]
#[ignore]
fn test_connect_db() {
    let conn = connect_db();
    assert!(conn.is_ok());
}
