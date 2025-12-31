use crate::{Config as WebConfig, handlers::build_axum_app, page::TemplateData};
use anyhow::Result;
use axum::Router;
use bon::bon;
use docs_rs_build_queue::AsyncBuildQueue;
use docs_rs_context::Context;
use docs_rs_database::{AsyncPoolClient, Config as DatabaseConfig, testing::TestDatabase};
use docs_rs_opentelemetry::testing::{CollectedMetrics, TestMetrics};
use docs_rs_registry_api::RegistryApi;
use docs_rs_storage::{AsyncStorage, Config as StorageConfig, StorageKind, testing::TestStorage};
use docs_rs_test_fakes::FakeRelease;
use std::sync::Arc;

pub(crate) struct TestEnvironment {
    pub(crate) context: Arc<Context>,
    pub(crate) config: Arc<WebConfig>,
    // so we can allow asserting collected metrics later.
    pub(crate) metrics: TestMetrics,
    #[allow(dead_code)] // we need to keep the storage so it can be cleaned up.
    pub(crate) storage: TestStorage,
    #[allow(dead_code)] // we need to keep the storage so it can be cleaned up.
    pub(crate) db: TestDatabase,
}

#[bon]
impl TestEnvironment {
    pub(crate) async fn new() -> Result<Self> {
        // NOTE: compiler crashes when I change the return to
        // `Self::builder().build().await
        #[allow(clippy::needless_question_mark)]
        Ok(Self::builder().build().await?)
    }

    #[builder(finish_fn = build)]
    pub(crate) async fn builder(
        web_config: Option<WebConfig>,
        registry_api_config: Option<docs_rs_registry_api::Config>,
        storage_config: Option<StorageConfig>,
    ) -> Result<Self> {
        docs_rs_logging::testing::init();

        let web_config = Arc::new(if let Some(web_config) = web_config {
            web_config
        } else {
            WebConfig::test_config()?
        });

        let registry_api_config =
            Arc::new(if let Some(registry_api_config) = registry_api_config {
                registry_api_config
            } else {
                docs_rs_registry_api::Config::from_environment()?
            });

        let registry_api = RegistryApi::from_config(&registry_api_config)?;

        let metrics = TestMetrics::new();

        let db_config = DatabaseConfig::test_config()?;
        let db = TestDatabase::new(&db_config, metrics.provider()).await?;

        let storage_config = Arc::new(if let Some(storage_config) = storage_config {
            storage_config
        } else {
            StorageConfig::test_config(StorageKind::Memory)?
        });

        let test_storage =
            TestStorage::from_config(storage_config.clone(), metrics.provider()).await?;

        Ok(Self {
            config: web_config,
            context: Context::builder()
                .with_runtime()
                .await?
                .meter_provider(metrics.provider().clone())
                .pool(db_config.into(), db.pool().clone())
                .storage(storage_config.clone(), test_storage.storage())
                .with_build_queue()?
                .registry_api(registry_api_config, registry_api.into())
                .with_build_limits()?
                .build()?
                .into(),
            db,
            storage: test_storage,
            metrics,
        })
    }

    pub(crate) fn config(&self) -> &WebConfig {
        &self.config
    }

    pub(crate) fn build_queue(&self) -> Result<&Arc<AsyncBuildQueue>> {
        self.context.build_queue()
    }

    pub(crate) async fn async_conn(&self) -> Result<AsyncPoolClient> {
        self.context.pool()?.get_async().await.map_err(Into::into)
    }

    pub(crate) fn storage(&self) -> Result<&Arc<AsyncStorage>> {
        self.context.storage()
    }

    pub(crate) async fn web_app(&self) -> Router {
        let template_data = Arc::new(TemplateData::new(1).unwrap());
        build_axum_app(self.config.clone(), self.context.clone(), template_data)
            .await
            .expect("could not build axum app")
    }

    pub async fn fake_release(&self) -> FakeRelease<'_> {
        FakeRelease::new(
            self.context.pool().unwrap().clone(),
            self.context.storage().unwrap().clone(),
        )
    }

    pub fn collected_metrics(&self) -> CollectedMetrics {
        self.metrics.collected_metrics()
    }
}
