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
#[derive(Clone)]
pub(crate) struct AppState(Arc<AppStateInner>);

struct AppStateInner {
    context: Arc<Context>,
    config: Arc<Config>,
    metrics: Arc<WebMetrics>,
    templates: Arc<TemplateData>,
}

impl AppState {
    pub(crate) fn new(
        config: Arc<Config>,
        context: Arc<Context>,
        templates: Arc<TemplateData>,
    ) -> Result<Self> {
        Self::validate_context(context.as_ref())?;

        Ok(Self(Arc::new(AppStateInner {
            metrics: Arc::new(WebMetrics::new(&context.meter_provider)),
            context,
            config,
            templates,
        })))
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
