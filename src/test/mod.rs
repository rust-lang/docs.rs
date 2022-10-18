mod fakes;

pub(crate) use self::fakes::FakeBuild;
use crate::cdn::CdnBackend;
use crate::db::{Pool, PoolClient};
use crate::error::Result;
use crate::repositories::RepositoryStatsUpdater;
use crate::storage::{Storage, StorageKind};
use crate::web::{cache, Server};
use crate::{BuildQueue, Config, Context, Index, Metrics};
use anyhow::Context as _;
use fn_error_context::context;
use iron::headers::CacheControl;
use once_cell::unsync::OnceCell;
use postgres::Client as Connection;
use reqwest::{
    blocking::{Client, ClientBuilder, RequestBuilder, Response},
    Method,
};
use std::{fs, net::SocketAddr, panic, sync::Arc, time::Duration};
use tokio::runtime::Runtime;
use tracing::{debug, error};

#[track_caller]
pub(crate) fn wrapper(f: impl FnOnce(&TestEnvironment) -> Result<()>) {
    let env = TestEnvironment::new();
    // if we didn't catch the panic, the server would hang forever
    let maybe_panic = panic::catch_unwind(panic::AssertUnwindSafe(|| f(&env)));
    env.cleanup();
    let result = match maybe_panic {
        Ok(r) => r,
        Err(payload) => panic::resume_unwind(payload),
    };

    if let Err(err) = result {
        eprintln!("the test failed: {}", err);
        for cause in err.chain() {
            eprintln!("  caused by: {}", cause);
        }

        eprintln!("{}", err.backtrace());

        panic!("the test failed");
    }
}

/// check a request if the cache control header matches NoCache
pub(crate) fn assert_no_cache(res: &Response) {
    assert_eq!(
        res.headers()
            .get("Cache-Control")
            .expect("missing cache-control header"),
        cache::NO_CACHE,
    );
}

/// check a request if the cache control header matches the given cache config.
pub(crate) fn assert_cache_control(
    res: &Response,
    cache_policy: cache::CachePolicy,
    config: &Config,
) {
    assert!(config.cache_control_stale_while_revalidate.is_some());
    let cache_control = res.headers().get("Cache-Control");

    let expected_directives = cache_policy.render(config);
    if expected_directives.is_empty() {
        assert!(cache_control.is_none());
    } else {
        assert_eq!(
            cache_control.expect("missing cache-control header"),
            &CacheControl(expected_directives).to_string()
        );
    }
}

/// Make sure that a URL returns a status code between 200-299
pub(crate) fn assert_success(path: &str, web: &TestFrontend) -> Result<()> {
    let status = web.get(path).send()?.status();
    assert!(status.is_success(), "failed to GET {}: {}", path, status);
    Ok(())
}

/// Make sure that a URL returns a status code between 200-299,
/// also check the cache-control headers.
pub(crate) fn assert_success_cached(
    path: &str,
    web: &TestFrontend,
    cache_policy: cache::CachePolicy,
    config: &Config,
) -> Result<()> {
    let response = web.get(path).send()?;
    assert_cache_control(&response, cache_policy, config);
    let status = response.status();
    assert!(status.is_success(), "failed to GET {}: {}", path, status);
    Ok(())
}

/// Make sure that a URL returns a 404
pub(crate) fn assert_not_found(path: &str, web: &TestFrontend) -> Result<()> {
    let response = web.get(path).send()?;

    // for now, 404s should always have `no-cache`
    assert_no_cache(&response);

    assert_eq!(
        response.status(),
        404,
        "GET {} should have been a 404",
        path
    );
    Ok(())
}

fn assert_redirect_common(
    path: &str,
    expected_target: &str,
    web: &TestFrontend,
) -> Result<Response> {
    let response = web.get_no_redirect(path).send()?;
    let status = response.status();
    if !status.is_redirection() {
        anyhow::bail!("non-redirect from GET {path}: {status}");
    }

    let mut redirect_target = response
        .headers()
        .get("Location")
        .context("missing 'Location' header")?
        .to_str()
        .context("non-ASCII redirect")?;

    if !expected_target.starts_with("http") {
        // TODO: Should be able to use Url::make_relative,
        // but https://github.com/servo/rust-url/issues/766
        let base = format!("http://{}", web.server_addr());
        redirect_target = redirect_target
            .strip_prefix(&base)
            .unwrap_or(redirect_target);
    }

    if redirect_target != expected_target {
        anyhow::bail!("got redirect to {redirect_target}");
    }

    Ok(response)
}

/// Makes sure that a URL redirects to a specific page, but doesn't check that the target exists
#[context("expected redirect from {path} to {expected_target}")]
pub(crate) fn assert_redirect_unchecked(
    path: &str,
    expected_target: &str,
    web: &TestFrontend,
) -> Result<()> {
    assert_redirect_common(path, expected_target, web).map(|_| ())
}

/// Make sure that a URL redirects to a specific page, and that the target exists and is not another redirect
#[context("expected redirect from {path} to {expected_target}")]
pub(crate) fn assert_redirect(path: &str, expected_target: &str, web: &TestFrontend) -> Result<()> {
    assert_redirect_common(path, expected_target, web)?;

    let response = web.get_no_redirect(expected_target).send()?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("failed to GET {expected_target}: {status}");
    }

    Ok(())
}

/// Make sure that a URL redirects to a specific page, and that the target exists and is not another redirect.
/// Also verifies that the redirect's cache-control header matches the provided cache policy.
#[context("expected redirect from {path} to {expected_target}")]
pub(crate) fn assert_redirect_cached(
    path: &str,
    expected_target: &str,
    cache_policy: cache::CachePolicy,
    web: &TestFrontend,
    config: &Config,
) -> Result<()> {
    let redirect_response = assert_redirect_common(path, expected_target, web)?;
    assert_cache_control(&redirect_response, cache_policy, config);

    let response = web.get_no_redirect(expected_target).send()?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("failed to GET {expected_target}: {status}");
    }

    Ok(())
}

pub(crate) struct TestEnvironment {
    build_queue: OnceCell<Arc<BuildQueue>>,
    config: OnceCell<Arc<Config>>,
    db: OnceCell<TestDatabase>,
    storage: OnceCell<Arc<Storage>>,
    cdn: OnceCell<Arc<CdnBackend>>,
    index: OnceCell<Arc<Index>>,
    runtime: OnceCell<Arc<Runtime>>,
    metrics: OnceCell<Arc<Metrics>>,
    frontend: OnceCell<TestFrontend>,
    repository_stats_updater: OnceCell<Arc<RepositoryStatsUpdater>>,
}

pub(crate) fn init_logger() {
    rustwide::logging::init_with(tracing_log::LogTracer::new());
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::DEBUG)
        .with_test_writer()
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}

impl TestEnvironment {
    fn new() -> Self {
        init_logger();
        Self {
            build_queue: OnceCell::new(),
            config: OnceCell::new(),
            db: OnceCell::new(),
            storage: OnceCell::new(),
            cdn: OnceCell::new(),
            index: OnceCell::new(),
            metrics: OnceCell::new(),
            frontend: OnceCell::new(),
            runtime: OnceCell::new(),
            repository_stats_updater: OnceCell::new(),
        }
    }

    fn cleanup(self) {
        if let Some(frontend) = self.frontend.into_inner() {
            frontend.server.leak();
        }
        if let Some(storage) = self.storage.get() {
            storage
                .cleanup_after_test()
                .expect("failed to cleanup after tests");
        }

        if let Some(config) = self.config.get() {
            if config.local_archive_cache_path.exists() {
                fs::remove_dir_all(&config.local_archive_cache_path).unwrap();
            }
        }
    }

    pub(crate) fn base_config(&self) -> Config {
        let mut config = Config::from_env().expect("failed to get base config");

        // create index directory
        fs::create_dir_all(config.registry_index_path.clone()).unwrap();

        // Use less connections for each test compared to production.
        config.max_pool_size = 4;
        config.min_pool_idle = 0;

        // Use the database for storage, as it's faster than S3.
        config.storage_backend = StorageKind::Database;

        // Use a temporary S3 bucket.
        config.s3_bucket = format!("docsrs-test-bucket-{}", rand::random::<u64>());
        config.s3_bucket_is_temporary = true;

        config.local_archive_cache_path =
            std::env::temp_dir().join(format!("docsrs-test-index-{}", rand::random::<u64>()));

        // set stale content serving so Cache::ForeverInCdn and Cache::ForeverInCdnAndStaleInBrowser
        // are actually different.
        config.cache_control_stale_while_revalidate = Some(86400);

        config
    }

    pub(crate) fn override_config(&self, f: impl FnOnce(&mut Config)) {
        let mut config = self.base_config();
        f(&mut config);

        if self.config.set(Arc::new(config)).is_err() {
            panic!("can't call override_config after the configuration is accessed!");
        }
    }

    pub(crate) fn build_queue(&self) -> Arc<BuildQueue> {
        self.build_queue
            .get_or_init(|| {
                Arc::new(BuildQueue::new(
                    self.db().pool(),
                    self.metrics(),
                    self.config(),
                    self.cdn(),
                    self.storage(),
                ))
            })
            .clone()
    }

    pub(crate) fn cdn(&self) -> Arc<CdnBackend> {
        self.cdn
            .get_or_init(|| Arc::new(CdnBackend::new(&self.config(), &self.runtime())))
            .clone()
    }

    pub(crate) fn config(&self) -> Arc<Config> {
        self.config
            .get_or_init(|| Arc::new(self.base_config()))
            .clone()
    }

    pub(crate) fn storage(&self) -> Arc<Storage> {
        self.storage
            .get_or_init(|| {
                Arc::new(
                    Storage::new(
                        self.db().pool(),
                        self.metrics(),
                        self.config(),
                        self.runtime(),
                    )
                    .expect("failed to initialize the storage"),
                )
            })
            .clone()
    }

    pub(crate) fn metrics(&self) -> Arc<Metrics> {
        self.metrics
            .get_or_init(|| Arc::new(Metrics::new().expect("failed to initialize the metrics")))
            .clone()
    }
    pub(crate) fn runtime(&self) -> Arc<Runtime> {
        self.runtime
            .get_or_init(|| Arc::new(Runtime::new().expect("failed to initialize runtime")))
            .clone()
    }

    pub(crate) fn index(&self) -> Arc<Index> {
        self.index
            .get_or_init(|| {
                Arc::new(
                    Index::new(self.config().registry_index_path.clone())
                        .expect("failed to initialize the index"),
                )
            })
            .clone()
    }

    pub(crate) fn repository_stats_updater(&self) -> Arc<RepositoryStatsUpdater> {
        self.repository_stats_updater
            .get_or_init(|| {
                Arc::new(RepositoryStatsUpdater::new(
                    &self.config(),
                    self.pool().expect("failed to get the pool"),
                ))
            })
            .clone()
    }

    pub(crate) fn db(&self) -> &TestDatabase {
        self.db.get_or_init(|| {
            TestDatabase::new(&self.config(), self.metrics()).expect("failed to initialize the db")
        })
    }

    pub(crate) fn override_frontend(&self, init: impl FnOnce(&mut TestFrontend)) -> &TestFrontend {
        let mut frontend = TestFrontend::new(self);
        init(&mut frontend);
        if self.frontend.set(frontend).is_err() {
            panic!("cannot call override_frontend after frontend is initialized");
        }
        self.frontend.get().unwrap()
    }

    pub(crate) fn frontend(&self) -> &TestFrontend {
        self.frontend.get_or_init(|| TestFrontend::new(self))
    }

    pub(crate) fn fake_release(&self) -> fakes::FakeRelease {
        fakes::FakeRelease::new(self.db(), self.storage())
    }
}

impl Context for TestEnvironment {
    fn config(&self) -> Result<Arc<Config>> {
        Ok(TestEnvironment::config(self))
    }

    fn build_queue(&self) -> Result<Arc<BuildQueue>> {
        Ok(TestEnvironment::build_queue(self))
    }

    fn storage(&self) -> Result<Arc<Storage>> {
        Ok(TestEnvironment::storage(self))
    }

    fn cdn(&self) -> Result<Arc<CdnBackend>> {
        Ok(TestEnvironment::cdn(self))
    }

    fn pool(&self) -> Result<Pool> {
        Ok(self.db().pool())
    }

    fn metrics(&self) -> Result<Arc<Metrics>> {
        Ok(self.metrics())
    }

    fn index(&self) -> Result<Arc<Index>> {
        Ok(self.index())
    }

    fn repository_stats_updater(&self) -> Result<Arc<RepositoryStatsUpdater>> {
        Ok(self.repository_stats_updater())
    }

    fn runtime(&self) -> Result<Arc<Runtime>> {
        Ok(self.runtime())
    }
}

pub(crate) struct TestDatabase {
    pool: Pool,
    schema: String,
}

impl TestDatabase {
    fn new(config: &Config, metrics: Arc<Metrics>) -> Result<Self> {
        // A random schema name is generated and used for the current connection. This allows each
        // test to create a fresh instance of the database to run within.
        let schema = format!("docs_rs_test_schema_{}", rand::random::<u64>());

        let pool = Pool::new_with_schema(config, metrics, &schema)?;

        let mut conn = Connection::connect(&config.database_url, postgres::NoTls)?;
        conn.batch_execute(&format!(
            "
                CREATE SCHEMA {0};
                SET search_path TO {0}, public;
            ",
            schema
        ))?;
        crate::db::migrate(None, &mut conn)?;

        // Move all sequence start positions 10000 apart to avoid overlapping primary keys
        let query: String = conn
            .query(
                "
                    SELECT relname
                    FROM pg_class
                    INNER JOIN pg_namespace
                        ON pg_class.relnamespace = pg_namespace.oid
                    WHERE pg_class.relkind = 'S'
                        AND pg_namespace.nspname = $1
                ",
                &[&schema],
            )?
            .into_iter()
            .map(|row| row.get(0))
            .enumerate()
            .map(|(i, sequence): (_, String)| {
                let offset = (i + 1) * 10000;
                format!(r#"ALTER SEQUENCE "{}" RESTART WITH {};"#, sequence, offset)
            })
            .collect();
        conn.batch_execute(&query)?;

        Ok(TestDatabase { pool, schema })
    }

    pub(crate) fn pool(&self) -> Pool {
        self.pool.clone()
    }

    pub(crate) fn conn(&self) -> PoolClient {
        self.pool
            .get()
            .expect("failed to get a connection out of the pool")
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let migration_result = crate::db::migrate(Some(0), &mut self.conn());
        if let Err(e) = self.conn().execute(
            format!("DROP SCHEMA {} CASCADE;", self.schema).as_str(),
            &[],
        ) {
            error!("failed to drop test schema {}: {}", self.schema, e);
        }
        // Drop the connection pool so we don't leak database connections
        self.pool.shutdown();
        migration_result.expect("downgrading database works");
    }
}

pub(crate) struct TestFrontend {
    server: Server,
    pub(crate) client: Client,
    pub(crate) client_no_redirect: Client,
}

impl TestFrontend {
    fn new(context: &dyn Context) -> Self {
        fn build(f: impl FnOnce(ClientBuilder) -> ClientBuilder) -> Client {
            let base = Client::builder()
                .connect_timeout(Duration::from_millis(2000))
                .timeout(Duration::from_millis(2000))
                // The test server only supports a single connection, so having two clients with
                // idle connections deadlocks the tests
                .pool_max_idle_per_host(0);
            f(base).build().unwrap()
        }

        Self {
            server: Server::start(Some("127.0.0.1:0"), context)
                .expect("failed to start the web server"),
            client: build(|b| b),
            client_no_redirect: build(|b| b.redirect(reqwest::redirect::Policy::none())),
        }
    }

    fn build_url(&self, url: &str) -> String {
        if url.is_empty() || url.starts_with('/') {
            format!("http://{}{}", self.server.addr(), url)
        } else {
            url.to_owned()
        }
    }

    pub(crate) fn server_addr(&self) -> SocketAddr {
        self.server.addr()
    }

    pub(crate) fn get(&self, url: &str) -> RequestBuilder {
        let url = self.build_url(url);
        debug!("getting {url}");
        self.client.request(Method::GET, url)
    }

    pub(crate) fn get_no_redirect(&self, url: &str) -> RequestBuilder {
        let url = self.build_url(url);
        debug!("getting {url} (no redirects)");
        self.client_no_redirect.request(Method::GET, url)
    }
}
