use anyhow::{Result, bail};
use docs_rs_config::AppConfig;
use docs_rs_env_vars::{env, maybe_env, require_env};
use docs_rs_rustwide::{BuildCores, CpuLimit, CpuQuota, SandboxImageSource};
use docs_rs_types::Duration;
use rustwide::cmd::DockerRuntime;
use std::{path::PathBuf, sync::Arc};

#[derive(Debug)]
pub struct Config {
    pub prefix: PathBuf,
    pub temp_dir: PathBuf,

    // Where to collect metrics for the metrics initiative.
    // When empty, we won't collect metrics.
    pub compiler_metrics_collection_path: Option<PathBuf>,

    pub build_workspace_reinitialization_interval: Duration,
    pub build_toolchain_update_interval: Duration,

    // Build params
    pub rustwide_workspace: PathBuf,
    pub inside_docker: bool,
    pub docker_image: Option<SandboxImageSource>,
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

        let build_cpu_limit: Option<CpuQuota> = maybe_env("DOCSRS_BUILD_CPU_LIMIT")?;
        let build_cpu_cores: Option<BuildCores> = maybe_env("DOCSRS_BUILD_CPU_CORES")?;

        if build_cpu_limit.is_some() && build_cpu_cores.is_some() {
            bail!("you only can define one of build_cpu_limit and build_cpu_cores");
        }

        let build_cpu_limit = build_cpu_cores
            .map(CpuLimit::Cores)
            .or(build_cpu_limit.map(CpuLimit::Quota));

        Ok(Self {
            temp_dir: prefix.join("tmp"),
            prefix,
            rustwide_workspace: env("DOCSRS_RUSTWIDE_WORKSPACE", PathBuf::from(".workspace"))?,
            inside_docker: env("DOCSRS_DOCKER", false)?,
            docker_image: maybe_env::<String>("DOCSRS_LOCAL_DOCKER_IMAGE")?
                .map(SandboxImageSource::local)
                .or(maybe_env::<String>("DOCSRS_DOCKER_IMAGE")?.map(SandboxImageSource::remote)),
            build_cpu_limit,
            include_default_targets: env("DOCSRS_INCLUDE_DEFAULT_TARGETS", true)?,
            disable_memory_limit: env("DOCSRS_DISABLE_MEMORY_LIMIT", false)?,
            build_workspace_reinitialization_interval: env(
                "DOCSRS_BUILD_WORKSPACE_REINITIALIZATION_INTERVAL",
                Duration::from_days(1),
            )?,
            build_toolchain_update_interval: env(
                "DOCSRS_BUILD_TOOLCHAIN_UPDATE_INTERVAL",
                Duration::from_hours(1),
            )?,
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
                ?image,
                "docker image from environment will be ignored for tests."
            )
        }

        config.include_default_targets = true;
        config.rustwide_workspace = docs_rs_rustwide::testing::test_workspace_path();
        config.docker_image = Some(docs_rs_rustwide::testing::test_sandbox_image());

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, process::Command};

    #[test]
    fn toolchain_update_interval_default_and_override() {
        const EXPECTED: &str = "DOCSRS_TEST_EXPECTED_TOOLCHAIN_INTERVAL";
        if let Ok(expected) = env::var(EXPECTED) {
            let config = Config::from_environment().unwrap();
            assert_eq!(
                config.build_toolchain_update_interval,
                expected.parse::<Duration>().unwrap()
            );
            return;
        }
        // Isolate environment changes in child processes so parallel tests are unaffected.
        for (value, expected) in [(None, "1h"), (Some("30m"), "30m"), (Some("0s"), "0s")] {
            let mut command = Command::new(env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "config::tests::toolchain_update_interval_default_and_override",
                    "--nocapture",
                ])
                .env("DOCSRS_PREFIX", env::temp_dir())
                .env_remove("DOCSRS_BUILD_CPU_CORES")
                .env_remove("DOCSRS_BUILD_CPU_LIMIT")
                .env_remove("DOCSRS_BUILD_TOOLCHAIN_UPDATE_INTERVAL")
                .env(EXPECTED, expected);
            if let Some(value) = value {
                command.env("DOCSRS_BUILD_TOOLCHAIN_UPDATE_INTERVAL", value);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        }
    }
}
