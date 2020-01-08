mod fakes;

use failure::Error;
use postgres::Connection;
use std::sync::{Arc, Mutex, MutexGuard};

pub(crate) fn with_database(f: impl FnOnce(&TestDatabase) -> Result<(), Error>) {
    let env = TestDatabase::new().expect("failed to initialize the environment");

    if let Err(err) = f(&env) {
        eprintln!("the test failed: {}", err);
        for cause in err.iter_causes() {
            eprintln!("  caused by: {}", cause);
        }

        eprintln!("{}", err.backtrace());

        panic!("the test failed");
    }
}

pub(crate) struct TestDatabase {
    conn: Arc<Mutex<Connection>>,
}

impl TestDatabase {
    fn new() -> Result<Self, Error> {
        // The temporary migration uses CREATE TEMPORARY TABLE instead of CREATE TABLE, creating
        // fresh temporary copies of the database on top of the real one. The temporary tables are
        // only visible to this connection, and will be deleted when it exits.
        let conn = crate::db::connect_db()?;
        crate::db::migrate_temporary(None, &conn)?;

        Ok(TestDatabase {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub(crate) fn conn(&self) -> MutexGuard<Connection> {
        self.conn.lock().expect("failed to lock the connection")
    }

    pub(crate) fn fake_release(&self) -> fakes::FakeRelease {
        fakes::FakeRelease::new(self)
    }
}
