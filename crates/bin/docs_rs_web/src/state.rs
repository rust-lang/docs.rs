use crate::{Config, metrics::WebMetrics, page::TemplateData};
use anyhow::{Result, anyhow};
use axum::extract::FromRef;
use docs_rs_build_queue::AsyncBuildQueue;
use docs_rs_context::Context;
use docs_rs_database::Pool;
use docs_rs_registry_api::RegistryApi;
use docs_rs_rustsec::RustsecClient;
use docs_rs_std_replacements::StdReplacements;
use docs_rs_storage::AsyncStorage;
use std::sync::Arc;

/// Web dependencies, reusing the services already initialized in the context.
/// Required services are checked at startup so extracting state is infallible.
#[derive(Clone)]
pub(crate) struct AppState(Arc<AppStateInner>);

struct AppStateInner {
    context: Arc<Context>,
    config: Arc<Config>,
    metrics: Arc<WebMetrics>,
    templates: Arc<TemplateData>,
}

impl AppState {
    pub(crate) fn new(config: Arc<Config>, context: Arc<Context>) -> Result<Self> {
        Self::validate_context(context.as_ref())?;
        let metrics = Arc::new(WebMetrics::new(&context.meter_provider));
        let templates = Arc::new(TemplateData::new(config.render_threads, metrics.clone())?);

        Ok(Self(Arc::new(AppStateInner {
            metrics,
            context,
            config,
            templates,
        })))
    }

    pub(crate) fn std_replacements(&self) -> Result<&Arc<StdReplacements>> {
        self.context()
            .std_replacements
            .as_ref()
            .ok_or_else(|| anyhow!("missing std_replacements client in AppState"))
    }

    pub(crate) fn rustsec(&self) -> Result<&Arc<RustsecClient>> {
        self.context()
            .rustsec
            .as_ref()
            .ok_or_else(|| anyhow!("missing rustsec client in AppState"))
    }
}

macro_rules! from_ref {
    ($ty:ty, $field:ident) => {
        impl AppState {
            #[allow(dead_code)]
            pub(crate) fn $field(&self) -> &$ty {
                &self.0.$field
            }
        }
        impl FromRef<AppState> for $ty {
            fn from_ref(state: &AppState) -> Self {
                state.0.$field.clone()
            }
        }
    };
}

from_ref!(Arc<Context>, context);
from_ref!(Arc<Config>, config);
from_ref!(Arc<WebMetrics>, metrics);
from_ref!(Arc<TemplateData>, templates);

macro_rules! context_services {
    ($($ty:ty => $method:ident),+ $(,)?) => {
        impl AppState {
            fn validate_context(context: &Context) -> Result<()> {
                $(context.$method()?;)+
                Ok(())
            }
        }

        $(
            impl AppState {
                pub(crate) fn $method(&self) -> &$ty {
                    &self
                        .0
                        .context
                        .$method()
                        .expect(concat!(
                            stringify!($method),
                            " was validated by AppState::new"
                        ))
                }
            }
            impl FromRef<AppState> for $ty {
                fn from_ref(state: &AppState) -> Self {
                    state.$method().clone()
                }
            }
        )+
    };
}

context_services!(
    Pool => pool,
    Arc<AsyncBuildQueue> => build_queue,
    Arc<RegistryApi> => registry_api,
    Arc<AsyncStorage> => storage,
);
