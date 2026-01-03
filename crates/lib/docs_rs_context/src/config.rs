use anyhow::{Result, anyhow};
use std::sync::Arc;

#[derive(Debug, Clone, Default, bon::Builder)]
pub struct Config {
    pub build_queue: Option<Arc<docs_rs_build_queue::Config>>,
    pub database: Option<Arc<docs_rs_database::Config>>,
    pub storage: Option<Arc<docs_rs_storage::Config>>,
    pub registry_api: Option<Arc<docs_rs_registry_api::Config>>,
    pub cdn: Option<Arc<docs_rs_fastly::Config>>,
    pub repository_stats: Option<Arc<docs_rs_repository_stats::Config>>,
    pub build_limits: Option<Arc<docs_rs_build_limits::Config>>,
}

impl Config {
    pub fn build_queue(&self) -> Result<&Arc<docs_rs_build_queue::Config>> {
        if let Some(ref build_queue) = self.build_queue {
            Ok(build_queue)
        } else {
            Err(anyhow!("build queue config is missing"))
        }
    }

    pub fn database(&self) -> Result<&Arc<docs_rs_database::Config>> {
        if let Some(ref database) = self.database {
            Ok(database)
        } else {
            Err(anyhow!("datbase config is missing"))
        }
    }

    pub fn storage(&self) -> Result<&Arc<docs_rs_storage::Config>> {
        if let Some(ref storage) = self.storage {
            Ok(storage)
        } else {
            Err(anyhow!("storage config is missing"))
        }
    }

    pub fn registry_api(&self) -> Result<&Arc<docs_rs_registry_api::Config>> {
        if let Some(ref registry_api) = self.registry_api {
            Ok(registry_api)
        } else {
            Err(anyhow!("registry api config is missing"))
        }
    }

    pub fn cdn(&self) -> Result<&Arc<docs_rs_fastly::Config>> {
        if let Some(ref cdn) = self.cdn {
            Ok(cdn)
        } else {
            Err(anyhow!("cdn config is missing"))
        }
    }

    pub fn build_limits(&self) -> Result<&Arc<docs_rs_build_limits::Config>> {
        if let Some(ref build_limits) = self.build_limits {
            Ok(build_limits)
        } else {
            Err(anyhow!("build limits config is missing"))
        }
    }

    pub fn repository_stats(&self) -> Result<&Arc<docs_rs_repository_stats::Config>> {
        if let Some(ref repository_stats) = self.repository_stats {
            Ok(repository_stats)
        } else {
            Err(anyhow!("repository stats config is missing"))
        }
    }
}
