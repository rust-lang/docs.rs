use crate::{Config, metrics::WebMetrics, page::TemplateData};
use anyhow::Result;
use axum::extract::FromRef;
use docs_rs_build_queue::AsyncBuildQueue;
use docs_rs_context::Context;
use docs_rs_database::Pool;
use docs_rs_registry_api::RegistryApi;
use docs_rs_storage::AsyncStorage;
use std::sync::Arc;

/// Web dependencies, reusing the services already initialized in the context.
/// Required services are checked at startup so extracting state is infallible.
#[derive(Clone, FromRef)]
pub(crate) struct AppState {
    pub(crate) context: Arc<Context>,
    pub(crate) config: Arc<Config>,
    pub(crate) pool: Pool,
    pub(crate) build_queue: Arc<AsyncBuildQueue>,
    pub(crate) registry_api: Arc<RegistryApi>,
    pub(crate) storage: Arc<AsyncStorage>,
    pub(crate) metrics: Arc<WebMetrics>,
    pub(crate) templates: Arc<TemplateData>,
}

impl AppState {
    pub(crate) fn new(
        config: Arc<Config>,
        context: Arc<Context>,
        templates: Arc<TemplateData>,
    ) -> Result<Self> {
        Ok(Self {
            pool: context.pool()?.clone(),
            build_queue: context.build_queue()?.clone(),
            registry_api: context.registry_api()?.clone(),
            storage: context.storage()?.clone(),
            metrics: Arc::new(WebMetrics::new(&context.meter_provider)),
            context,
            config,
            templates,
        })
    }
}
