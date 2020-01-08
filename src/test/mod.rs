mod fakes;

use crate::web::Server;
use failure::Error;
use postgres::Connection;
use std::sync::{Arc, Mutex, MutexGuard};
use reqwest::{Client, Method, RequestBuilder};

pub(crate) fn with_database(f: impl FnOnce(&TestDatabase) -> Result<(), Error>) {
    let env = TestDatabase::new().expect("failed to initialize the environment");
    report_error(|| f(&env));
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

pub(crate) fn with_frontend(db: &TestDatabase, f: impl FnOnce(&TestFrontend) -> Result<(), Error>) {
    let frontend = TestFrontend::new(db);
    let result = f(&frontend);
    frontend.server.leak();
    report_error(|| result)
}

pub(crate) struct TestFrontend {
    server: Server,
    client: Client,
}

impl TestFrontend {
    fn new(db: &TestDatabase) -> Self {
        Self {
            server: Server::start_test(db.conn.clone()),
            client: Client::new(),
        }
    }

    fn build_request(&self, method: Method, url: &str) -> RequestBuilder {
        self.client.request(method, &format!("http://{}{}", self.server.addr(), url))
    }

    pub(crate) fn get(&self, url: &str) -> RequestBuilder {
        self.build_request(Method::GET, url)
    }
}

fn report_error(f: impl FnOnce() -> Result<(), Error>) {
    if let Err(err) = f() {
        eprintln!("the test failed: {}", err);
        for cause in err.iter_causes() {
            eprintln!("  caused by: {}", cause);
        }

        eprintln!("{}", err.backtrace());

        panic!("the test failed");
    }
}
