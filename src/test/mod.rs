mod fakes;

use crate::web::Server;
use failure::Error;
use once_cell::unsync::OnceCell;
use postgres::Connection;
use reqwest::{Client, Method, RequestBuilder};
use std::sync::{Arc, Mutex, MutexGuard};

pub(crate) fn wrapper(f: impl FnOnce(&TestEnvironment) -> Result<(), Error>) {
    let env = TestEnvironment::new();
    let result = f(&env);
    env.cleanup();

    if let Err(err) = result {
        eprintln!("the test failed: {}", err);
        for cause in err.iter_causes() {
            eprintln!("  caused by: {}", cause);
        }

        eprintln!("{}", err.backtrace());

        panic!("the test failed");
    }
}

pub(crate) struct TestEnvironment {
    db: OnceCell<TestDatabase>,
    frontend: OnceCell<TestFrontend>,
}

impl TestEnvironment {
    fn new() -> Self {
        Self {
            db: OnceCell::new(),
            frontend: OnceCell::new(),
        }
    }

    fn cleanup(self) {
        if let Some(frontend) = self.frontend.into_inner() {
            frontend.server.leak();
        }
    }

    pub(crate) fn db(&self) -> &TestDatabase {
        self.db
            .get_or_init(|| TestDatabase::new().expect("failed to initialize the db"))
    }

    pub(crate) fn frontend(&self) -> &TestFrontend {
        self.frontend.get_or_init(|| TestFrontend::new(self.db()))
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
        self.client
            .request(method, &format!("http://{}{}", self.server.addr(), url))
    }

    pub(crate) fn get(&self, url: &str) -> RequestBuilder {
        self.build_request(Method::GET, url)
    }
}
