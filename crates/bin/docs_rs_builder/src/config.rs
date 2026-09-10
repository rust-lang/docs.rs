use anyhow::{Result, bail};
use docs_rs_config::AppConfig;
use docs_rs_env_vars::{env, maybe_env, require_env};
use docs_rs_rustwide::{BuildCores, CpuLimit};
use rustwide::cmd::DockerRuntime;
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
    /// Docker CPU limit
    /// Either quota, or assigned cores.
    pub build_cpu_limit: Option<CpuLimit>,
    pub include_default_targets: bool,
    pub disable_memory_limit: bool,
    /// Docker runtime the builder should use.
    pub docker_runtime: DockerRuntime,

    // other module configs
    pub build_limits: Arc<docs_rs_build_limits::Config>,
}

impl AppConfig for Config {
    fn from_environment() -> Result<Self> {
        let prefix: PathBuf = require_env("DOCSRS_PREFIX")?;

        let build_cpu_limit: Option<f32> = maybe_env("DOCSRS_BUILD_CPU_LIMIT")?;
        let build_cpu_cores: Option<BuildCores> = maybe_env("DOCSRS_BUILD_CPU_CORES")?;

        if build_cpu_limit.is_some() && build_cpu_cores.is_some() {
            bail!("you only can define one of build_cpu_limit and build_cpu_cores");
        }

        Ok(Self {
            temp_dir: prefix.join("tmp"),
            prefix,
            rustwide_workspace: env("DOCSRS_RUSTWIDE_WORKSPACE", PathBuf::from(".workspace"))?,
            inside_docker: env("DOCSRS_DOCKER", false)?,
            docker_image: maybe_env("DOCSRS_LOCAL_DOCKER_IMAGE")?
                .or(maybe_env("DOCSRS_DOCKER_IMAGE")?),
            build_cpu_limit: build_cpu_cores
                .map(CpuLimit::Cores)
                .or(build_cpu_limit.map(CpuLimit::Quota)),
            include_default_targets: env("DOCSRS_INCLUDE_DEFAULT_TARGETS", true)?,
            disable_memory_limit: env("DOCSRS_DISABLE_MEMORY_LIMIT", false)?,
            build_workspace_reinitialization_interval: Duration::from_secs(env(
                "DOCSRS_BUILD_WORKSPACE_REINITIALIZATION_INTERVAL",
                86400,
            )?),
            compiler_metrics_collection_path: maybe_env("DOCSRS_COMPILER_METRICS_PATH")?,
            docker_runtime: maybe_env("DOCSRS_DOCKER_RUNTIME")?.unwrap_or_default(),
            build_limits: Arc::new(docs_rs_build_limits::Config::from_environment()?),
        })
    }

    #[cfg(test)]
    fn test_config() -> Result<Self> {
        let mut config = Self::from_environment()?;

        if let Some(image) = config.docker_image {
            tracing::warn!(
                image,
                "docker image from environment will be ignored for tests."
            )
        }

        config.include_default_targets = true;
        config.rustwide_workspace = docs_rs_rustwide::testing::test_workspace_path();
        config.docker_image = Some(docs_rs_rustwide::SANDBOX_IMAGE_LINUX_MICRO.into());

        Ok(config)
    }
}
