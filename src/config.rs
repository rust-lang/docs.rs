use anyhow::{Context as _, Result};
use docs_rs_context::Config as NewConfig;
use std::sync::Arc;

#[derive(Debug, derive_builder::Builder)]
#[builder(pattern = "owned")]
pub struct Config {
    pub(crate) fastly: Arc<docs_rs_fastly::Config>,
    pub(crate) opentelemetry: Arc<docs_rs_opentelemetry::Config>,
    pub(crate) registry_api: Arc<docs_rs_registry_api::Config>,
    pub(crate) database: Arc<docs_rs_database::Config>,
    pub(crate) repository_stats: Arc<docs_rs_repository_stats::Config>,
    pub(crate) storage: Arc<docs_rs_storage::Config>,
    pub(crate) build_queue: Arc<docs_rs_build_queue::Config>,
    pub(crate) build_limits: Arc<docs_rs_build_limits::Config>,
    pub builder: Arc<docs_rs_builder::Config>,
    pub watcher: Arc<docs_rs_watcher::Config>,
    pub web: Arc<docs_rs_web::Config>,
}

impl From<&Config> for NewConfig {
    fn from(value: &Config) -> Self {
        Self {
            build_queue: Some(value.build_queue.clone()),
            database: Some(value.database.clone()),
            storage: Some(value.storage.clone()),
            registry_api: Some(value.registry_api.clone()),
            cdn: value.fastly.is_valid().then(|| value.fastly.clone()),
            repository_stats: Some(value.repository_stats.clone()),
            build_limits: Some(value.build_limits.clone()),
        }
    }
}

impl Config {
    pub fn from_env() -> Result<ConfigBuilder> {
        Ok(ConfigBuilder::default()
            .fastly(Arc::new(
                docs_rs_fastly::Config::from_environment()
                    .context("error reading fastly config from environment")?,
            ))
            .opentelemetry(Arc::new(docs_rs_opentelemetry::Config::from_environment()?))
            .registry_api(Arc::new(docs_rs_registry_api::Config::from_environment()?))
            .database(Arc::new(docs_rs_database::Config::from_environment()?))
            .repository_stats(Arc::new(
                docs_rs_repository_stats::Config::from_environment()?,
            ))
            .storage(Arc::new(docs_rs_storage::Config::from_environment()?))
            .build_queue(Arc::new(docs_rs_build_queue::Config::from_environment()?))
            .build_limits(Arc::new(docs_rs_build_limits::Config::from_environment()?))
            .builder(Arc::new(docs_rs_builder::Config::from_environment()?))
            .watcher(Arc::new(docs_rs_watcher::Config::from_environment()?))
            .web(Arc::new(docs_rs_web::Config::from_environment()?)))
    }
}
