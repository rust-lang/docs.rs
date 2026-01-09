use anyhow::Result;
use docs_rs_config::AppConfig;

pub(crate) struct DummyConfig;

impl AppConfig for DummyConfig {
    fn from_environment() -> Result<Self> {
        Ok(Self {})
    }
}

pub(crate) type TestEnvironment = docs_rs_context::testing::TestEnvironment<DummyConfig>;
