use docs_rs_env_vars::{env, maybe_env, require_env};
use std::{path::PathBuf, sync::Arc, time::Duration};

#[derive(Debug)]
pub struct Config {
    pub prefix: PathBuf,
    pub temp_dir: PathBuf,

    // Where to collect metrics for the metrics initiative.
    // When empty, we won't collect metrics.
    pub compiler_metrics_collection_path: Option<PathBuf>,

    pub build_workspace_reinitialization_interval: Duration,

    // Build params
    pub rustwide_workspace: PathBuf,
    pub inside_docker: bool,
    pub docker_image: Option<String>,
    pub build_cpu_limit: Option<u32>,
    pub include_default_targets: bool,
    pub disable_memory_limit: bool,

    // other module configs
    pub build_limits: Arc<docs_rs_build_limits::Config>,
}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {
        let prefix: PathBuf = require_env("DOCSRS_PREFIX")?;
        Ok(Self {
            temp_dir: prefix.join("tmp"),
            prefix,
            rustwide_workspace: env("DOCSRS_RUSTWIDE_WORKSPACE", PathBuf::from(".workspace"))?,
            inside_docker: env("DOCSRS_DOCKER", false)?,
            docker_image: maybe_env("DOCSRS_LOCAL_DOCKER_IMAGE")?
                .or(maybe_env("DOCSRS_DOCKER_IMAGE")?),

            build_cpu_limit: maybe_env("DOCSRS_BUILD_CPU_LIMIT")?,
            include_default_targets: env("DOCSRS_INCLUDE_DEFAULT_TARGETS", true)?,
            disable_memory_limit: env("DOCSRS_DISABLE_MEMORY_LIMIT", false)?,
            build_workspace_reinitialization_interval: Duration::from_secs(env(
                "DOCSRS_BUILD_WORKSPACE_REINITIALIZATION_INTERVAL",
                86400,
            )?),
            compiler_metrics_collection_path: maybe_env("DOCSRS_COMPILER_METRICS_PATH")?,
            build_limits: Arc::new(docs_rs_build_limits::Config::from_environment()?),
        })
    }

    #[cfg(any(feature = "testing", test))]
    pub fn test_config() -> anyhow::Result<Self> {
        let mut config = Self::from_environment()?;

        config.include_default_targets = true;

        Ok(config)
    }
}
