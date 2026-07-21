use crate::{Config as BuilderConfig, RustwideBuilder};
use anyhow::Result;
use docs_rs_config::AppConfig as _;

pub(crate) type TestEnvironment = docs_rs_context::testing::BlockingTestEnvironment<BuilderConfig>;

pub(crate) trait TestEnvironmentExt {
    fn build_builder(&self) -> Result<RustwideBuilder>;
}

impl TestEnvironmentExt for TestEnvironment {
    fn build_builder(&self) -> Result<RustwideBuilder> {
        crate::logging::init(&docs_rs_logging::Config::test_config()?); // initialize rustwide logging
        RustwideBuilder::init(self.config().clone(), self)
    }
}
