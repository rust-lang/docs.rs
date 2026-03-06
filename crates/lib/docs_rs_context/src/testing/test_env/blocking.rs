use super::non_blocking::TestEnvironment;
use anyhow::{Context as _, Result};
use bon::bon;
use docs_rs_config::AppConfig;
use docs_rs_storage::Config as StorageConfig;
use std::ops::Deref;
use tokio::runtime;

pub struct BlockingTestEnvironment<C> {
    inner: TestEnvironment<C>,
    #[allow(dead_code)] // we need to keep the runtime alive while using the inner environment
    runtime: runtime::Runtime,
}

impl<C: AppConfig> Deref for BlockingTestEnvironment<C> {
    type Target = TestEnvironment<C>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[bon]
impl<C: AppConfig> BlockingTestEnvironment<C> {
    pub fn new() -> Result<Self> {
        // NOTE: compiler crashes when I change the return to
        // `Self::builder().build()`
        #[allow(clippy::needless_question_mark)]
        Ok(Self::builder().build()?)
    }

    #[builder(finish_fn = build)]
    pub fn builder(
        config: Option<C>,
        registry_api_config: Option<docs_rs_registry_api::Config>,
        storage_config: Option<StorageConfig>,
    ) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("failed to initialize runtime")?;

        Ok(Self {
            inner: runtime.block_on(
                TestEnvironment::builder()
                    .maybe_config(config)
                    .maybe_registry_api_config(registry_api_config)
                    .maybe_storage_config(storage_config)
                    .build(),
            )?,
            runtime,
        })
    }
}
