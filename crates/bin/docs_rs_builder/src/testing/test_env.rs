use crate::{Config as BuilderConfig, RustwideBuilder};
use anyhow::Result;
use docs_rs_config::AppConfig as _;
use docs_rs_rustwide::testing::TestWorkspace;
use std::ops::{Deref, DerefMut};

pub(crate) type TestEnvironment = docs_rs_context::testing::BlockingTestEnvironment<BuilderConfig>;

pub(crate) struct TestBuilder {
    builder: RustwideBuilder,
    _workspace: TestWorkspace,
}

impl Deref for TestBuilder {
    type Target = RustwideBuilder;

    fn deref(&self) -> &Self::Target {
        &self.builder
    }
}

impl DerefMut for TestBuilder {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.builder
    }
}

pub(crate) trait TestEnvironmentExt {
    fn build_builder(&self) -> Result<TestBuilder>;
}

impl TestEnvironmentExt for TestEnvironment {
    fn build_builder(&self) -> Result<TestBuilder> {
        crate::logging::init(&docs_rs_logging::Config::test_config()?); // initialize rustwide logging
        let workspace = TestWorkspace::acquire_at(&self.config().rustwide_workspace)?;
        let builder = RustwideBuilder::init(self.config().clone(), self)?;
        Ok(TestBuilder {
            builder,
            _workspace: workspace,
        })
    }
}
