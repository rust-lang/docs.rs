mod fakes;

pub(crate) use self::fakes::{fake_release_that_failed_before_build, FakeBuild};
use crate::cdn::CdnBackend;
use crate::db::{self, AsyncPoolClient, Pool};
use crate::error::Result;
use crate::repositories::RepositoryStatsUpdater;
use crate::storage::{AsyncStorage, Storage, StorageKind};
use crate::web::{build_axum_app, cache, page::TemplateData};
use crate::{
    AsyncBuildQueue, BuildQueue, Config, Context, Index, InstanceMetrics, RegistryApi,
    ServiceMetrics,
};
use anyhow::Context as _;
use axum::body::Bytes;
use axum::{async_trait, body::Body, http::Request, response::Response as AxumResponse, Router};
use fn_error_context::context;
use futures_util::{stream::TryStreamExt, FutureExt};
use http_body_util::BodyExt; // for `collect`
use once_cell::sync::OnceCell;
use serde::de::DeserializeOwned;
use sqlx::Connection as _;
use std::{fs, future::Future, panic, rc::Rc, str::FromStr, sync::Arc};
use tokio::runtime::{Builder, Runtime};
use tower::ServiceExt;
use tracing::error;

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

pub(crate) trait AxumResponseTestExt {
    async fn text(self) -> Result<String>;
    async fn bytes(self) -> Result<Bytes>;
    async fn json<T: DeserializeOwned>(self) -> Result<T>;
    fn redirect_target(&self) -> Option<&str>;
    fn assert_cache_control(&self, cache_policy: cache::CachePolicy, config: &Config);
    fn error_for_status(self) -> Result<Self>
    where
        Self: Sized;
}

impl AxumResponseTestExt for axum::response::Response {
    async fn text(self) -> Result<String> {
        Ok(String::from_utf8_lossy(&(self.bytes().await?)).to_string())
    }
    async fn bytes(self) -> Result<Bytes> {
        Ok(self.into_body().collect().await?.to_bytes())
    }
    async fn json<T: DeserializeOwned>(self) -> Result<T> {
        let body = self.text().await?;
        Ok(serde_json::from_str(&body)?)
    }
    fn redirect_target(&self) -> Option<&str> {
        self.headers().get("Location")?.to_str().ok()
    }
    fn assert_cache_control(&self, cache_policy: cache::CachePolicy, config: &Config) {
        assert!(config.cache_control_stale_while_revalidate.is_some());
        let cache_control = self.headers().get("Cache-Control");

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

    fn error_for_status(self) -> Result<Self>
    where
        Self: Sized,
    {
        let status = self.status();
        if status.is_client_error() || status.is_server_error() {
            anyhow::bail!("got status code {}", status);
        } else {
            Ok(self)
        }
    }
}

pub(crate) trait AxumRouterTestExt {
    async fn get_and_follow_redirects(&self, path: &str) -> Result<AxumResponse>;
    async fn assert_redirect_cached_unchecked(
        &self,
        path: &str,
        expected_target: &str,
        cache_policy: cache::CachePolicy,
        config: &Config,
    ) -> Result<AxumResponse>;
    async fn assert_not_found(&self, path: &str) -> Result<()>;
    async fn assert_success_cached(
        &self,
        path: &str,
        cache_policy: cache::CachePolicy,
        config: &Config,
    ) -> Result<()>;
    async fn assert_success(&self, path: &str) -> Result<AxumResponse>;
    async fn get(&self, path: &str) -> Result<AxumResponse>;
    async fn post(&self, path: &str) -> Result<AxumResponse>;
    async fn assert_redirect_common(
        &self,
        path: &str,
        expected_target: &str,
    ) -> Result<AxumResponse>;
    async fn assert_redirect(&self, path: &str, expected_target: &str) -> Result<AxumResponse>;
    async fn assert_redirect_unchecked(
        &self,
        path: &str,
        expected_target: &str,
    ) -> Result<AxumResponse>;
    async fn assert_redirect_cached(
        &self,
        path: &str,
        expected_target: &str,
        cache_policy: cache::CachePolicy,
        config: &Config,
    ) -> Result<AxumResponse>;
}

impl AxumRouterTestExt for axum::Router {
    /// Make sure that a URL returns a status code between 200-299
    async fn assert_success(&self, path: &str) -> Result<AxumResponse> {
        let response = self.get(path).await?;

        let status = response.status();
        if status.is_redirection() {
            panic!(
                "expected success response from {path}, got redirect ({status}) to {:?}",
                response.redirect_target()
            );
        }
        assert!(status.is_success(), "failed to GET {path}: {status}");
        Ok(response)
    }

    async fn assert_not_found(&self, path: &str) -> Result<()> {
        let response = self.get(path).await?;

        // for now, 404s should always have `no-cache`
        // assert_no_cache(&response);
        assert_eq!(
            response
                .headers()
                .get("Cache-Control")
                .expect("missing cache-control header")
                .to_str()
                .unwrap(),
            cache::NO_CACHING.to_str().unwrap(),
        );

        assert_eq!(response.status(), 404, "GET {path} should have been a 404");
        Ok(())
    }

    async fn assert_success_cached(
        &self,
        path: &str,
        cache_policy: cache::CachePolicy,
        config: &Config,
    ) -> Result<()> {
        let response = self.get(path).await?;
        let status = response.status();
        assert!(
            status.is_success(),
            "failed to GET {path}: {status} (redirect: {})",
            response.redirect_target().unwrap_or_default()
        );
        response.assert_cache_control(cache_policy, config);
        Ok(())
    }

    async fn get(&self, path: &str) -> Result<AxumResponse> {
        Ok(self
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await?)
    }

    async fn get_and_follow_redirects(&self, path: &str) -> Result<AxumResponse> {
        let mut path = path.to_owned();
        for _ in 0..=10 {
            let response = self.get(&path).await?;
            if response.status().is_redirection() {
                if let Some(target) = response.redirect_target() {
                    path = target.to_owned();
                    continue;
                }
            }
            return Ok(response);
        }
        panic!("redirect loop");
    }

    async fn post(&self, path: &str) -> Result<AxumResponse> {
        Ok(self
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await?)
    }

    async fn assert_redirect_common(
        &self,
        path: &str,
        expected_target: &str,
    ) -> Result<AxumResponse> {
        let response = self.get(path).await?;
        let status = response.status();
        if !status.is_redirection() {
            anyhow::bail!("non-redirect from GET {path}: {status}");
        }

        let redirect_target = response
            .redirect_target()
            .context("missing 'Location' header")?;

        // FIXME: not sure we need this
        // if !expected_target.starts_with("http") {
        //     // TODO: Should be able to use Url::make_relative,
        //     // but https://github.com/servo/rust-url/issues/766
        //     let base = format!("http://{}", web.server_addr());
        //     redirect_target = redirect_target
        //         .strip_prefix(&base)
        //         .unwrap_or(redirect_target);
        // }

        if redirect_target != expected_target {
            anyhow::bail!("got redirect to {redirect_target}");
        }

        Ok(response)
    }

    #[context("expected redirect from {path} to {expected_target}")]
    async fn assert_redirect(&self, path: &str, expected_target: &str) -> Result<AxumResponse> {
        let redirect_response = self.assert_redirect_common(path, expected_target).await?;

        let response = self.get(expected_target).await?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("failed to GET {expected_target}: {status}");
        }

        Ok(redirect_response)
    }

    async fn assert_redirect_unchecked(
        &self,
        path: &str,
        expected_target: &str,
    ) -> Result<AxumResponse> {
        self.assert_redirect_common(path, expected_target).await
    }

    async fn assert_redirect_cached(
        &self,
        path: &str,
        expected_target: &str,
        cache_policy: cache::CachePolicy,
        config: &Config,
    ) -> Result<AxumResponse> {
        let redirect_response = self.assert_redirect_common(path, expected_target).await?;
        redirect_response.assert_cache_control(cache_policy, config);

        let response = self.get(expected_target).await?;
        let status = response.status();
        if !status.is_success() {
            anyhow::bail!("failed to GET {expected_target}: {status}");
        }

        Ok(redirect_response)
    }

    async fn assert_redirect_cached_unchecked(
        &self,
        path: &str,
        expected_target: &str,
        cache_policy: cache::CachePolicy,
        config: &Config,
    ) -> Result<AxumResponse> {
        let redirect_response = self.assert_redirect_common(path, expected_target).await?;
        redirect_response.assert_cache_control(cache_policy, config);
        Ok(redirect_response)
    }
}

pub(crate) struct TestEnvironment {
    build_queue: OnceCell<Arc<BuildQueue>>,
    async_build_queue: tokio::sync::OnceCell<Arc<AsyncBuildQueue>>,
    config: OnceCell<Arc<Config>>,
    db: tokio::sync::OnceCell<TestDatabase>,
    storage: OnceCell<Arc<Storage>>,
    async_storage: tokio::sync::OnceCell<Arc<AsyncStorage>>,
    cdn: tokio::sync::OnceCell<Arc<CdnBackend>>,
    index: OnceCell<Arc<Index>>,
    registry_api: OnceCell<Arc<RegistryApi>>,
    runtime: OnceCell<Arc<Runtime>>,
    instance_metrics: OnceCell<Arc<InstanceMetrics>>,
    service_metrics: OnceCell<Arc<ServiceMetrics>>,
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
            async_build_queue: tokio::sync::OnceCell::new(),
            config: OnceCell::new(),
            db: tokio::sync::OnceCell::new(),
            storage: OnceCell::new(),
            async_storage: tokio::sync::OnceCell::new(),
            cdn: tokio::sync::OnceCell::new(),
            index: OnceCell::new(),
            registry_api: OnceCell::new(),
            instance_metrics: OnceCell::new(),
            service_metrics: OnceCell::new(),
            runtime: OnceCell::new(),
            repository_stats_updater: OnceCell::new(),
        }
    }

    fn cleanup(self) {
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
        config.max_pool_size = 8;
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

    pub(crate) async fn async_build_queue(&self) -> Arc<AsyncBuildQueue> {
        self.async_build_queue
            .get_or_init(|| async {
                Arc::new(AsyncBuildQueue::new(
                    self.async_db().await.pool(),
                    self.instance_metrics(),
                    self.config(),
                    self.async_storage().await,
                ))
            })
            .await
            .clone()
    }

    pub(crate) fn build_queue(&self) -> Arc<BuildQueue> {
        let runtime = self.runtime();
        self.build_queue
            .get_or_init(|| {
                Arc::new(BuildQueue::new(
                    runtime.clone(),
                    runtime.block_on(self.async_build_queue()),
                ))
            })
            .clone()
    }

    pub(crate) async fn cdn(&self) -> Arc<CdnBackend> {
        self.cdn
            .get_or_init(|| async { Arc::new(CdnBackend::new(&self.config()).await) })
            .await
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

    pub(crate) fn fake_release(&self) -> fakes::FakeRelease {
        self.runtime().block_on(self.async_fake_release())
    }

    pub(crate) async fn web_app(&self) -> Router {
        let template_data = Arc::new(TemplateData::new(1).unwrap());
        build_axum_app(self, template_data)
            .await
            .expect("could not build axum app")
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

    async fn async_build_queue(&self) -> Result<Arc<AsyncBuildQueue>> {
        Ok(TestEnvironment::async_build_queue(self).await)
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

    async fn cdn(&self) -> Result<Arc<CdnBackend>> {
        Ok(TestEnvironment::cdn(self).await)
    }

    async fn async_pool(&self) -> Result<Pool> {
        Ok(self.async_db().await.pool())
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
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        self.runtime.block_on(async {
            let mut conn = self.async_conn().await;
            let migration_result = db::migrate(&mut conn, Some(0)).await;

            if let Err(e) = sqlx::query(format!("DROP SCHEMA {} CASCADE;", self.schema).as_str())
                .execute(&mut *conn)
                .await
            {
                error!("failed to drop test schema {}: {}", self.schema, e);
            }

            migration_result.expect("downgrading database works");
        });
    }
}
