use crate::{Config as WebConfig, handlers::build_axum_app, page::TemplateData};
use axum::Router;
use std::sync::Arc;

pub(crate) type TestEnvironment = docs_rs_context::testing::TestEnvironment<WebConfig>;

pub(crate) trait TestEnvironmentExt {
    async fn web_app(&self) -> Router;
}

impl TestEnvironmentExt for TestEnvironment {
    async fn web_app(&self) -> Router {
        let template_data = Arc::new(TemplateData::new(1).unwrap());
        build_axum_app(self.config().clone(), self.context().clone(), template_data)
            .await
            .expect("could not build axum app")
    }
}
