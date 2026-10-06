use crate::{Config as WebConfig, handlers::build_axum_app};
use axum::Router;

pub(crate) type TestEnvironment = docs_rs_context::testing::TestEnvironment<WebConfig>;

pub(crate) trait TestEnvironmentExt {
    async fn web_app(&self) -> Router;
}

impl TestEnvironmentExt for TestEnvironment {
    async fn web_app(&self) -> Router {
        build_axum_app(self.config().clone(), self.context().clone())
            .await
            .expect("could not build axum app")
    }
}
