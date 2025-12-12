use docs_rs_env_vars::{env, maybe_env, require_env};
use std::{path::PathBuf, time::Duration};

#[derive(Debug)]
pub struct Config {
    pub(crate) prefix: PathBuf,
    pub(crate) temp_dir: PathBuf,

    // Where to collect metrics for the metrics initiative.
    // When empty, we won't collect metrics.
    pub(crate) compiler_metrics_collection_path: Option<PathBuf>,

    pub(crate) build_workspace_reinitialization_interval: Duration,

    // Build params
    pub(crate) build_attempts: u16,
    pub(crate) delay_between_build_attempts: Duration,
    pub(crate) rustwide_workspace: PathBuf,
    pub(crate) inside_docker: bool,
    pub(crate) docker_image: Option<String>,
    pub(crate) build_cpu_limit: Option<u32>,
    pub(crate) build_default_memory_limit: Option<usize>,
    pub(crate) include_default_targets: bool,
    pub(crate) disable_memory_limit: bool,
}

impl Config {
    pub fn from_environment() -> anyhow::Result<Self> {
        let prefix: PathBuf = require_env("DOCSRS_PREFIX")?;
        Ok(Self {
            temp_dir: prefix.join("tmp"),
            // api_host: env(
            //     "DOCSRS_FASTLY_API_HOST",
            //     "https://api.fastly.com".parse().unwrap(),
            // )?,
            // api_token: maybe_env("DOCSRS_FASTLY_API_TOKEN")?,
            // service_sid: maybe_env("DOCSRS_FASTLY_SERVICE_SID_WEB")?,
            prefix,
            build_attempts: env("DOCSRS_BUILD_ATTEMPTS", 5u16)?,
            delay_between_build_attempts: Duration::from_secs(env::<u64>(
                "DOCSRS_DELAY_BETWEEN_BUILD_ATTEMPTS",
                60,
            )?),
            rustwide_workspace: env("DOCSRS_RUSTWIDE_WORKSPACE", PathBuf::from(".workspace"))?,
            inside_docker: env("DOCSRS_DOCKER", false)?,
            docker_image: maybe_env("DOCSRS_LOCAL_DOCKER_IMAGE")?
                .or(maybe_env("DOCSRS_DOCKER_IMAGE")?),

            build_cpu_limit: maybe_env("DOCSRS_BUILD_CPU_LIMIT")?,
            build_default_memory_limit: maybe_env("DOCSRS_BUILD_DEFAULT_MEMORY_LIMIT")?,
            include_default_targets: env("DOCSRS_INCLUDE_DEFAULT_TARGETS", true)?,
            disable_memory_limit: env("DOCSRS_DISABLE_MEMORY_LIMIT", false)?,
            build_workspace_reinitialization_interval: Duration::from_secs(env(
                "DOCSRS_BUILD_WORKSPACE_REINITIALIZATION_INTERVAL",
                86400,
            )?),
            compiler_metrics_collection_path: maybe_env("DOCSRS_COMPILER_METRICS_PATH")?,
        })
    }
}
