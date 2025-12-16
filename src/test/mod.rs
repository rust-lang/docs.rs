mod fakes;
pub(crate) mod headers;

pub(crate) use self::fakes::{FakeBuild, fake_release_that_failed_before_build};
use crate::{
    AsyncBuildQueue, BuildQueue, Config, Context,
    config::ConfigBuilder,
    db::{self, AsyncPoolClient, Pool},
    error::Result,
    storage::{AsyncStorage, Storage, StorageKind},
    web::{build_axum_app, cache, page::TemplateData},
};
use anyhow::{Context as _, anyhow};
use axum::body::Bytes;
use axum::{Router, body::Body, http::Request, response::Response as AxumResponse};
use axum_extra::headers::{ETag, HeaderMapExt as _};
use docs_rs_fastly::Cdn;
use docs_rs_headers::{IfNoneMatch, SURROGATE_CONTROL, SurrogateKeys};
use docs_rs_opentelemetry::{
    AnyMeterProvider,
    testing::{CollectedMetrics, setup_test_meter_provider},
};
use docs_rs_types::Version;
use fn_error_context::context;
use futures_util::stream::TryStreamExt;
use http::{
    HeaderMap, HeaderName, HeaderValue, StatusCode,
    header::{CACHE_CONTROL, CONTENT_TYPE},
};
use http_body_util::BodyExt;
use opentelemetry_sdk::metrics::InMemoryMetricExporter;
use serde::de::DeserializeOwned;
use sqlx::Connection as _;
use std::{collections::HashMap, fs, future::Future, panic, rc::Rc, str::FromStr, sync::Arc};
use tokio::{runtime, task::block_in_place};
use tower::ServiceExt;
use tracing::error;

// testing krate name constants
pub(crate) const KRATE: &str = "krate";
// some versions as constants for tests
pub(crate) const V0_1: Version = Version::new(0, 1, 0);
pub(crate) const V1: Version = Version::new(1, 0, 0);
pub(crate) const V2: Version = Version::new(2, 0, 0);
pub(crate) const V3: Version = Version::new(3, 0, 0);

pub(crate) fn async_wrapper<F, Fut>(f: F)
where
    F: FnOnce(Rc<TestEnvironment>) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let env = Rc::new(
        TestEnvironment::with_config_and_runtime(TestEnvironment::base_config().build().unwrap())
            .unwrap(),
    );

    env.runtime().block_on(f(env.clone())).expect("test failed");
}

pub(crate) fn assert_cache_headers_eq(
    response: &axum::response::Response,
    expected_headers: &cache::ResponseCacheHeaders,
) {
    assert_eq!(
        expected_headers.cache_control.as_ref(),
        response.headers().get(CACHE_CONTROL),
        "cache control header mismatch"
    );
    assert_eq!(
        expected_headers.surrogate_control.as_ref(),
        response.headers().get(&SURROGATE_CONTROL),
        "surrogate control header mismatch"
    );
    assert_eq!(
        expected_headers.surrogate_keys.as_ref(),
        response.headers().typed_get::<SurrogateKeys>().as_ref(),
        "surrogate key header mismatch"
    );
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

        // This method is only about asserting if the handler did set the right _policy_.
        assert_cache_headers_eq(self, &cache_policy.render(config).unwrap());
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
    async fn get_with_headers<F>(&self, path: &str, f: F) -> Result<AxumResponse>
    where
        F: FnOnce(&mut HeaderMap);
    async fn get_and_follow_redirects(&self, path: &str) -> Result<AxumResponse>;
    async fn assert_redirect_cached_unchecked(
        &self,
        path: &str,
        expected_target: &str,
        cache_policy: cache::CachePolicy,
        config: &Config,
    ) -> Result<AxumResponse>;
    async fn assert_not_found(&self, path: &str) -> Result<()>;
    async fn assert_conditional_get(
        &self,
        initial_path: &str,
        uncached_response: &AxumResponse,
    ) -> Result<()>;
    async fn assert_success_and_conditional_get(&self, path: &str) -> Result<()>;

    async fn assert_success_cached(
        &self,
        path: &str,
        cache_policy: cache::CachePolicy,
        config: &Config,
    ) -> Result<AxumResponse>;
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

    async fn assert_conditional_get(
        &self,
        initial_path: &str,
        uncached_response: &AxumResponse,
    ) -> Result<()> {
        let etag: ETag = uncached_response
            .headers()
            .typed_get()
            .ok_or_else(|| anyhow!("missing ETag header"))?;

        let if_none_match = IfNoneMatch::from(etag.clone());

        // general rule:
        //
        // if a header influences how any client or intermediate proxy should treat the response,
        // it should be repeated on the 304 response.
        //
        // This logic assumes _all_ headers have to be repeated, except for a few known ones.
        const NON_CACHE_HEADERS: &[&HeaderName] = &[&CONTENT_TYPE];

        // store original headers, to assert that they are repeated on the 304 response.
        let original_headers: HashMap<HeaderName, HeaderValue> = uncached_response
            .headers()
            .iter()
            .filter(|(k, _)| !NON_CACHE_HEADERS.contains(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        {
            let cached_response = self
                .get_with_headers(initial_path, |headers| {
                    headers.typed_insert(if_none_match);
                })
                .await?;
            assert_eq!(cached_response.status(), StatusCode::NOT_MODIFIED);

            // most headers are repeated on the 304 response.
            let cached_response_headers: HashMap<HeaderName, HeaderValue> = cached_response
                .headers()
                .iter()
                .filter_map(|(k, v)| {
                    if original_headers.contains_key(k) {
                        Some((k.clone(), v.clone()))
                    } else {
                        None
                    }
                })
                .collect();

            assert_eq!(original_headers, cached_response_headers);
        }
        Ok(())
    }

    async fn assert_success_and_conditional_get(&self, path: &str) -> Result<()> {
        self.assert_conditional_get(path, &self.assert_success(path).await?)
            .await
    }

    async fn assert_not_found(&self, path: &str) -> Result<()> {
        let response = self.get(path).await?;

        // for now, 404s should always have `no-cache`
        assert_cache_headers_eq(&response, &cache::NO_CACHING);

        assert_eq!(response.status(), 404, "GET {path} should have been a 404");
        Ok(())
    }

    async fn assert_success_cached(
        &self,
        path: &str,
        cache_policy: cache::CachePolicy,
        config: &Config,
    ) -> Result<AxumResponse> {
        let response = self.get(path).await?;
        let status = response.status();
        assert!(
            status.is_success(),
            "failed to GET {path}: {status} (redirect: {})",
            response.redirect_target().unwrap_or_default()
        );
        response.assert_cache_control(cache_policy, config);
        Ok(response)
    }

    async fn get(&self, path: &str) -> Result<AxumResponse> {
        Ok(self
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await?)
    }

    async fn get_with_headers<F>(&self, path: &str, f: F) -> Result<AxumResponse>
    where
        F: FnOnce(&mut HeaderMap),
    {
        let mut builder = Request::builder().uri(path);
        f(builder.headers_mut().unwrap());

        Ok(self
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await?)
    }

    async fn get_and_follow_redirects(&self, path: &str) -> Result<AxumResponse> {
        let mut path = path.to_owned();
        for _ in 0..=10 {
            let response = self.get(&path).await?;
            if response.status().is_redirection()
                && let Some(target) = response.redirect_target()
            {
                path = target.to_owned();
                continue;
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
            anyhow::bail!(
                "got redirect to `{redirect_target}`, expected redirect to `{expected_target}`",
            );
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
    // NOTE: the database has to come before the context,
    // otherwise it can happen that we can't cleanup the test database
    // because the tokio runtime from the context is gone.
    db: TestDatabase,
    pub context: Context,
    owned_runtime: Option<Arc<runtime::Runtime>>,
    collected_metrics: InMemoryMetricExporter,
}

pub(crate) fn init_logger() {
    use tracing_subscriber::{EnvFilter, filter::Directive};

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
    pub(crate) fn new_with_runtime() -> Result<Self> {
        Self::with_config_and_runtime(Self::base_config().build()?)
    }

    pub(crate) async fn new() -> Result<Self> {
        Self::with_config(Self::base_config().build()?).await
    }

    pub(crate) fn with_config_and_runtime(config: Config) -> Result<Self> {
        let runtime = Arc::new(
            runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("failed to initialize runtime")?,
        );
        let mut env = runtime.block_on(Self::with_config(config))?;
        env.owned_runtime = Some(runtime);
        Ok(env)
    }

    pub(crate) async fn with_config(config: Config) -> Result<Self> {
        init_logger();

        // create index directory
        fs::create_dir_all(config.registry_index_path.clone())?;

        let (metric_exporter, meter_provider) = setup_test_meter_provider();

        let test_db = TestDatabase::new(&config, &meter_provider)
            .await
            .context("can't initialize test database")?;

        Ok(Self {
            context: Context::from_test_config(config, meter_provider, test_db.pool().clone())
                .await?,
            db: test_db,
            owned_runtime: None,
            collected_metrics: metric_exporter,
        })
    }

    pub(crate) fn base_config() -> ConfigBuilder {
        Config::from_env()
            .expect("can't load base config from environment")
            // Use less connections for each test compared to production.
            .max_pool_size(8)
            .min_pool_idle(2)
            // Use the database for storage, as it's faster than S3.
            .storage_backend(StorageKind::Database)
            // Use a temporary S3 bucket.
            .s3_bucket(format!("docsrs-test-bucket-{}", rand::random::<u64>()))
            .s3_bucket_is_temporary(true)
            .local_archive_cache_path(
                std::env::temp_dir().join(format!("docsrs-test-index-{}", rand::random::<u64>())),
            )
            // set stale content serving so Cache::ForeverInCdn and Cache::ForeverInCdnAndStaleInBrowser
            // are actually different.
            .cache_control_stale_while_revalidate(Some(86400))
            .include_default_targets(true)
    }

    pub(crate) fn async_build_queue(&self) -> &AsyncBuildQueue {
        &self.context.async_build_queue
    }

    pub(crate) fn build_queue(&self) -> &BuildQueue {
        &self.context.build_queue
    }

    pub(crate) fn cdn(&self) -> &Cdn {
        self.context
            .cdn
            .as_ref()
            .expect("in test envs we always have the mock CDN")
    }

    pub(crate) fn config(&self) -> &Config {
        &self.context.config
    }

    pub(crate) fn async_storage(&self) -> &AsyncStorage {
        &self.context.async_storage
    }

    pub(crate) fn storage(&self) -> &Storage {
        &self.context.storage
    }

    pub(crate) fn runtime(&self) -> &runtime::Handle {
        &self.context.runtime
    }

    pub(crate) fn async_db(&self) -> &TestDatabase {
        &self.db
    }

    pub(crate) fn collected_metrics(&self) -> CollectedMetrics {
        self.context.meter_provider.force_flush().unwrap();
        CollectedMetrics(self.collected_metrics.get_finished_metrics().unwrap())
    }

    pub(crate) async fn web_app(&self) -> Router {
        let template_data = Arc::new(TemplateData::new(1).unwrap());
        build_axum_app(&self.context, template_data)
            .await
            .expect("could not build axum app")
    }

    pub(crate) async fn fake_release(&self) -> fakes::FakeRelease<'_> {
        fakes::FakeRelease::new(self.async_db(), self.context.async_storage.clone())
    }
}

impl Drop for TestEnvironment {
    fn drop(&mut self) {
        let storage = self.context.storage.clone();
        let runtime = self.runtime();

        block_in_place(move || {
            runtime.block_on(async move {
                storage
                    .cleanup_after_test()
                    .await
                    .expect("failed to cleanup after tests");
            });
        });

        if self.context.config.local_archive_cache_path.exists() {
            fs::remove_dir_all(&self.context.config.local_archive_cache_path).unwrap();
        }
    }
}

#[derive(Debug)]
pub(crate) struct TestDatabase {
    pool: Pool,
    schema: String,
    runtime: runtime::Handle,
}

impl TestDatabase {
    async fn new(config: &Config, otel_meter_provider: &AnyMeterProvider) -> Result<Self> {
        // A random schema name is generated and used for the current connection. This allows each
        // test to create a fresh instance of the database to run within.
        let schema = format!("docs_rs_test_schema_{}", rand::random::<u64>());

        let pool = Pool::new_with_schema(config, &schema, otel_meter_provider).await?;

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

        Ok(TestDatabase {
            pool,
            schema,
            runtime: runtime::Handle::current(),
        })
    }

    pub(crate) fn pool(&self) -> &Pool {
        &self.pool
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
        let pool = self.pool.clone();
        let schema = self.schema.clone();
        let runtime = self.runtime.clone();

        block_in_place(move || {
            runtime.block_on(async move {
                let Ok(mut conn) = pool.get_async().await else {
                    error!("error in drop impl");
                    return;
                };

                let migration_result = db::migrate(&mut conn, Some(0)).await;

                if let Err(e) = sqlx::query(format!("DROP SCHEMA {} CASCADE;", schema).as_str())
                    .execute(&mut *conn)
                    .await
                {
                    error!("failed to drop test schema {}: {}", schema, e);
                    return;
                }

                if let Err(err) = migration_result {
                    error!(?err, "error reverting migrations");
                }
            })
        });
    }
}
