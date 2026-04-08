use crate::{AsyncStorage, Config, StorageKind};
use anyhow::Result;
use docs_rs_opentelemetry::AnyMeterProvider;
use std::{ops::Deref, sync::Arc};
use tokio::{runtime, task::block_in_place};

pub struct TestStorage {
    runtime: runtime::Handle,
    config: Arc<Config>,
    storage: Arc<AsyncStorage>,
}

impl Deref for TestStorage {
    type Target = AsyncStorage;

    fn deref(&self) -> &Self::Target {
        &self.storage
    }
}

impl TestStorage {
    pub async fn from_kind(kind: StorageKind, meter_provider: &AnyMeterProvider) -> Result<Self> {
        Self::from_config(
            Arc::new(Config::test_config_with_kind(kind)?),
            meter_provider,
        )
        .await
    }

    pub async fn from_config(
        config: Arc<Config>,
        meter_provider: &AnyMeterProvider,
    ) -> Result<Self> {
        let storage = Arc::new(AsyncStorage::new(config.clone(), meter_provider).await?);
        let runtime = runtime::Handle::current();

        Ok(Self {
            config,
            runtime,
            storage,
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn storage(&self) -> Arc<AsyncStorage> {
        self.storage.clone()
    }
}

impl Drop for TestStorage {
    fn drop(&mut self) {
        let storage = self.storage.clone();
        let runtime = self.runtime.clone();

        block_in_place(move || {
            runtime.block_on(async move {
                storage
                    .cleanup_after_test()
                    .await
                    .expect("failed to cleanup after tests");
            });
        });

        if self.config.archive_index_cache.path.exists() {
            std::fs::remove_dir_all(&self.config.archive_index_cache.path).unwrap();
        }
    }
}
