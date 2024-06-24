mod fakes;

pub(crate) use self::fakes::FakeBuild;
use crate::cdn::CdnBackend;
use crate::db::{self, AsyncPoolClient, Pool, PoolClient};
use crate::error::Result;
use crate::repositories::RepositoryStatsUpdater;
use crate::storage::{AsyncStorage, Storage, StorageKind};
use crate::web::{build_axum_app, cache, page::TemplateData};
use crate::{BuildQueue, Config, Context, Index, InstanceMetrics, RegistryApi, ServiceMetrics};
use anyhow::Context as _;
use axum::async_trait;
use fn_error_context::context;
use futures_util::{stream::TryStreamExt, FutureExt};
use once_cell::sync::OnceCell;
use reqwest::{
    blocking::{Client, ClientBuilder, RequestBuilder, Response},
    Method,
};
use sqlx::Connection as _;
use std::thread::{self, JoinHandle};
use std::{
    fs, future::Future, net::SocketAddr, panic, rc::Rc, str::FromStr, sync::Arc, time::Duration,
};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::oneshot::Sender;
use tracing::{debug, error, instrument, trace};

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
        eprintln!("the test failed: {err}");
        for cause in err.chain() {
            eprintln!("  caused by: {cause}");
        }

        eprintln!("{}", err.backtrace());

        panic!("the test failed");
    }
}

pub(crate) fn async_wrapper<F, Fut>(f: F)
where
    F: FnOnce(Rc<TestEnvironment>) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let env = Rc::new(TestEnvironment::new());

    let fut = f(env.clone());

    let runtime = env.runtime();

    // if we didn't catch the panic, the server would hang forever
    let maybe_panic = runtime.block_on(panic::AssertUnwindSafe(fut).catch_unwind());

    let env = Rc::into_inner(env).unwrap();
    env.cleanup();

    let result = match maybe_panic {
        Ok(r) => r,
        Err(payload) => panic::resume_unwind(payload),
    };

    if let Err(err) = result {
        eprintln!("the test failed: {err}");
        for cause in err.chain() {
            eprintln!("  caused by: {cause}");
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
            .expect("missing cache-control header")
            .to_str()
            .unwrap(),
        cache::NO_CACHING.to_str().unwrap(),
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

    if let Some(expected_directives) = cache_policy.render(config) {
        assert_eq!(
            cache_control
                .expect("missing cache-control header")
                .to_str()
                .unwrap(),
            expected_directives.to_str().unwrap(),
        );
    } else {
        assert!(cache_control.is_none());
    }
}

/// Make sure that a URL returns a status code between 200-299
pub(crate) fn assert_success(path: &str, web: &TestFrontend) -> Result<()> {
    let status = web.get(path).send()?.status();
    assert!(status.is_success(), "failed to GET {path}: {status}");
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
    let status = response.status();
    assert!(status.is_success(), "failed to GET {path}: {status}");
    assert_cache_control(&response, cache_policy, config);
    Ok(())
}

/// Make sure that a URL returns a 404
pub(crate) fn assert_not_found(path: &str, web: &TestFrontend) -> Result<()> {
    let response = web.get(path).send()?;

    // for now, 404s should always have `no-cache`
    assert_no_cache(&response);

    assert_eq!(response.status(), 404, "GET {path} should have been a 404");
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
///
/// Returns the redirect response
#[context("expected redirect from {path} to {expected_target}")]
pub(crate) fn assert_redirect_unchecked(
    path: &str,
    expected_target: &str,
    web: &TestFrontend,
) -> Result<Response> {
    assert_redirect_common(path, expected_target, web)
}

/// Makes sure that a URL redirects to a specific page, but doesn't check that the target exists
///
/// Returns the redirect response
#[context("expected redirect from {path} to {expected_target}")]
pub(crate) fn assert_redirect_cached_unchecked(
    path: &str,
    expected_target: &str,
    cache_policy: cache::CachePolicy,
    web: &TestFrontend,
    config: &Config,
) -> Result<Response> {
    let redirect_response = assert_redirect_common(path, expected_target, web)?;
    assert_cache_control(&redirect_response, cache_policy, config);
    Ok(redirect_response)
}

/// Make sure that a URL redirects to a specific page, and that the target exists and is not another redirect
///
/// Returns the redirect response
#[context("expected redirect from {path} to {expected_target}")]
pub(crate) fn assert_redirect(
    path: &str,
    expected_target: &str,
    web: &TestFrontend,
) -> Result<Response> {
    let redirect_response = assert_redirect_common(path, expected_target, web)?;

    let response = web.get_no_redirect(expected_target).send()?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("failed to GET {expected_target}: {status}");
    }

    Ok(redirect_response)
}

/// Make sure that a URL redirects to a specific page, and that the target exists and is not another redirect.
/// Also verifies that the redirect's cache-control header matches the provided cache policy.
///
/// Returns the redirect response
#[context("expected redirect from {path} to {expected_target}")]
pub(crate) fn assert_redirect_cached(
    path: &str,
    expected_target: &str,
    cache_policy: cache::CachePolicy,
    web: &TestFrontend,
    config: &Config,
) -> Result<Response> {
    let redirect_response = assert_redirect_common(path, expected_target, web)?;
    assert_cache_control(&redirect_response, cache_policy, config);

    let response = web.get_no_redirect(expected_target).send()?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("failed to GET {expected_target}: {status}");
    }

    Ok(redirect_response)
}

pub(crate) struct TestEnvironment {
    build_queue: OnceCell<Arc<BuildQueue>>,
    config: OnceCell<Arc<Config>>,
    db: tokio::sync::OnceCell<TestDatabase>,
    storage: OnceCell<Arc<Storage>>,
    async_storage: tokio::sync::OnceCell<Arc<AsyncStorage>>,
    cdn: OnceCell<Arc<CdnBackend>>,
    index: OnceCell<Arc<Index>>,
    registry_api: OnceCell<Arc<RegistryApi>>,
    runtime: OnceCell<Arc<Runtime>>,
    instance_metrics: OnceCell<Arc<InstanceMetrics>>,
    service_metrics: OnceCell<Arc<ServiceMetrics>>,
    frontend: OnceCell<TestFrontend>,
    repository_stats_updater: OnceCell<Arc<RepositoryStatsUpdater>>,
}

pub(crate) fn init_logger() {
    use tracing_subscriber::{filter::Directive, EnvFilter};

    rustwide::logging::init_with(tracing_log::LogTracer::new());
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(Directive::from_str("docs_rs=info").unwrap())
                .with_env_var("DOCSRS_LOG")
                .from_env_lossy(),
        )
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
            db: tokio::sync::OnceCell::new(),
            storage: OnceCell::new(),
            async_storage: tokio::sync::OnceCell::new(),
            cdn: OnceCell::new(),
            index: OnceCell::new(),
            registry_api: OnceCell::new(),
            instance_metrics: OnceCell::new(),
            service_metrics: OnceCell::new(),
            frontend: OnceCell::new(),
            runtime: OnceCell::new(),
            repository_stats_updater: OnceCell::new(),
        }
    }

    fn cleanup(self) {
        if let Some(frontend) = self.frontend.into_inner() {
            frontend.shutdown();
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
        config.max_legacy_pool_size = 4;
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

        config.include_default_targets = true;

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
                    self.instance_metrics(),
                    self.config(),
                    self.storage(),
                    self.runtime(),
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

    pub(crate) async fn async_storage(&self) -> Arc<AsyncStorage> {
        self.async_storage
            .get_or_init(|| async {
                let db = self.async_db().await;
                Arc::new(
                    AsyncStorage::new(db.pool(), self.instance_metrics(), self.config())
                        .await
                        .expect("failed to initialize the async storage"),
                )
            })
            .await
            .clone()
    }

    pub(crate) fn storage(&self) -> Arc<Storage> {
        let runtime = self.runtime();
        self.storage
            .get_or_init(|| {
                Arc::new(Storage::new(
                    runtime.block_on(self.async_storage()),
                    runtime,
                ))
            })
            .clone()
    }

    pub(crate) fn instance_metrics(&self) -> Arc<InstanceMetrics> {
        self.instance_metrics
            .get_or_init(|| {
                Arc::new(InstanceMetrics::new().expect("failed to initialize the instance metrics"))
            })
            .clone()
    }

    pub(crate) fn service_metrics(&self) -> Arc<ServiceMetrics> {
        self.service_metrics
            .get_or_init(|| {
                Arc::new(ServiceMetrics::new().expect("failed to initialize the service metrics"))
            })
            .clone()
    }

    pub(crate) fn runtime(&self) -> Arc<Runtime> {
        self.runtime
            .get_or_init(|| {
                Arc::new(
                    Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("failed to initialize runtime"),
                )
            })
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

    pub(crate) fn registry_api(&self) -> Arc<RegistryApi> {
        self.registry_api
            .get_or_init(|| {
                Arc::new(
                    RegistryApi::new(
                        self.config().registry_api_host.clone(),
                        self.config().crates_io_api_call_retries,
                    )
                    .expect("failed to initialize the registry api"),
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
        self.runtime().block_on(self.async_db())
    }

    pub(crate) async fn async_db(&self) -> &TestDatabase {
        self.db
            .get_or_init(|| async {
                let config = self.config();
                let runtime = self.runtime();
                let instance_metrics = self.instance_metrics();
                self.runtime()
                    .spawn_blocking(move || TestDatabase::new(&config, runtime, instance_metrics))
                    .await
                    .unwrap()
                    .expect("failed to initialize the db")
            })
            .await
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
        self.runtime().block_on(self.async_fake_release())
    }

    pub(crate) async fn async_fake_release(&self) -> fakes::FakeRelease {
        fakes::FakeRelease::new(
            self.async_db().await,
            self.async_storage().await,
            self.runtime(),
        )
    }
}

#[async_trait]
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

    async fn async_storage(&self) -> Result<Arc<AsyncStorage>> {
        Ok(TestEnvironment::async_storage(self).await)
    }

    fn cdn(&self) -> Result<Arc<CdnBackend>> {
        Ok(TestEnvironment::cdn(self))
    }

    fn pool(&self) -> Result<Pool> {
        Ok(self.db().pool())
    }

    fn instance_metrics(&self) -> Result<Arc<InstanceMetrics>> {
        Ok(self.instance_metrics())
    }

    fn service_metrics(&self) -> Result<Arc<ServiceMetrics>> {
        Ok(self.service_metrics())
    }

    fn index(&self) -> Result<Arc<Index>> {
        Ok(self.index())
    }

    fn registry_api(&self) -> Result<Arc<RegistryApi>> {
        Ok(self.registry_api())
    }

    fn repository_stats_updater(&self) -> Result<Arc<RepositoryStatsUpdater>> {
        Ok(self.repository_stats_updater())
    }

    fn runtime(&self) -> Result<Arc<Runtime>> {
        Ok(self.runtime())
    }
}

#[derive(Debug)]
pub(crate) struct TestDatabase {
    pool: Pool,
    schema: String,
    runtime: Arc<Runtime>,
}

impl TestDatabase {
    fn new(config: &Config, runtime: Arc<Runtime>, metrics: Arc<InstanceMetrics>) -> Result<Self> {
        // A random schema name is generated and used for the current connection. This allows each
        // test to create a fresh instance of the database to run within.
        let schema = format!("docs_rs_test_schema_{}", rand::random::<u64>());

        let pool = Pool::new_with_schema(config, runtime.clone(), metrics, &schema)?;

        runtime.block_on({
            let schema = schema.clone();
            async move {
                let mut conn = sqlx::PgConnection::connect(&config.database_url).await?;
                sqlx::query(&format!("CREATE SCHEMA {schema}"))
                    .execute(&mut conn)
                    .await
                    .context("error creating schema")?;
                sqlx::query(&format!("SET search_path TO {schema}, public"))
                    .execute(&mut conn)
                    .await
                    .context("error setting search path")?;
                db::migrate(&mut conn, None)
                    .await
                    .context("error running migrations")?;

                // Move all sequence start positions 10000 apart to avoid overlapping primary keys
                let sequence_names: Vec<_> = sqlx::query!(
                    "SELECT relname
                     FROM pg_class
                     INNER JOIN pg_namespace ON
                         pg_class.relnamespace = pg_namespace.oid
                     WHERE pg_class.relkind = 'S'
                         AND pg_namespace.nspname = $1
                    ",
                    schema,
                )
                .fetch(&mut conn)
                .map_ok(|row| row.relname)
                .try_collect()
                .await?;

                for (i, sequence) in sequence_names.into_iter().enumerate() {
                    let offset = (i + 1) * 10000;
                    sqlx::query(&format!(
                        r#"ALTER SEQUENCE "{sequence}" RESTART WITH {offset};"#
                    ))
                    .execute(&mut conn)
                    .await?;
                }

                Ok::<(), anyhow::Error>(())
            }
        })?;

        Ok(TestDatabase {
            pool,
            schema,
            runtime,
        })
    }

    pub(crate) fn pool(&self) -> Pool {
        self.pool.clone()
    }

    pub(crate) async fn async_conn(&self) -> AsyncPoolClient {
        self.pool
            .get_async()
            .await
            .expect("failed to get a connection out of the pool")
    }

    pub(crate) fn conn(&self) -> PoolClient {
        self.pool
            .get()
            .expect("failed to get a connection out of the pool")
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let migration_result = self.runtime.block_on(async {
            let mut conn = self.async_conn().await;
            db::migrate(&mut conn, Some(0)).await
        });

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
    axum_server_thread: JoinHandle<()>,
    axum_server_shutdown_signal: Sender<()>,
    axum_server_address: SocketAddr,
    pub(crate) client: Client,
    pub(crate) client_no_redirect: Client,
}

impl TestFrontend {
    #[instrument(skip_all)]
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

        debug!("loading template data");
        let template_data = Arc::new(TemplateData::new(1).unwrap());

        let runtime = context.runtime().unwrap();

        debug!("binding local TCP port for axum");
        let axum_listener = runtime
            .block_on(tokio::net::TcpListener::bind(
                "127.0.0.1:0".parse::<SocketAddr>().unwrap(),
            ))
            .unwrap();

        let axum_addr = axum_listener.local_addr().unwrap();
        debug!("bound to local address: {}", axum_addr);

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        debug!("building axum app");
        let axum_app = build_axum_app(context, template_data).expect("could not build axum app");

        let handle = thread::spawn({
            let runtime = context.runtime().unwrap();
            move || {
                runtime.block_on(async {
                    axum::serve(axum_listener, axum_app.into_make_service())
                        .with_graceful_shutdown(async {
                            rx.await.ok();
                        })
                        .await
                        .expect("error from axum server")
                })
            }
        });

        Self {
            axum_server_address: axum_addr,
            axum_server_thread: handle,
            axum_server_shutdown_signal: tx,
            client: build(|b| b),
            client_no_redirect: build(|b| b.redirect(reqwest::redirect::Policy::none())),
        }
    }

    #[instrument(skip_all)]
    fn shutdown(self) {
        trace!("sending axum shutdown signal");
        self.axum_server_shutdown_signal
            .send(())
            .expect("could not send shutdown signal");

        trace!("joining axum server thread");
        self.axum_server_thread
            .join()
            .expect("could not join axum background thread");
    }

    fn build_url(&self, url: &str) -> String {
        if url.is_empty() || url.starts_with('/') {
            format!("http://{}{}", self.axum_server_address, url)
        } else {
            url.to_owned()
        }
    }

    pub(crate) fn server_addr(&self) -> SocketAddr {
        self.axum_server_address
    }

    pub(crate) fn get(&self, url: &str) -> RequestBuilder {
        let url = self.build_url(url);
        debug!("getting {url}");
        self.client.request(Method::GET, url)
    }

    pub(crate) fn post(&self, url: &str) -> RequestBuilder {
        let url = self.build_url(url);
        debug!("posting {url}");
        self.client.request(Method::POST, url)
    }

    pub(crate) fn get_no_redirect(&self, url: &str) -> RequestBuilder {
        let url = self.build_url(url);
        debug!("getting {url} (no redirects)");
        self.client_no_redirect.request(Method::GET, url)
    }
}
