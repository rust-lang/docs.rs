use anyhow::{Result, bail};
use docs_rs_config::AppConfig;
use docs_rs_env_vars::{env, maybe_env, require_env};
use std::{
    num::ParseIntError,
    ops::{Deref, RangeInclusive},
    path::PathBuf,
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use thiserror::Error;

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
    /// Docker CPU quota / CPU count.
    pub build_cpu_limit: Option<u32>,
    /// CPU cores the builder should use.
    pub build_cpu_cores: Option<BuildCores>,
    pub include_default_targets: bool,
    pub disable_memory_limit: bool,

    // other module configs
    pub build_limits: Arc<docs_rs_build_limits::Config>,
}

impl AppConfig for Config {
    fn from_environment() -> Result<Self> {
        let prefix: PathBuf = require_env("DOCSRS_PREFIX")?;
        let config = Self {
            temp_dir: prefix.join("tmp"),
            prefix,
            rustwide_workspace: env("DOCSRS_RUSTWIDE_WORKSPACE", PathBuf::from(".workspace"))?,
            inside_docker: env("DOCSRS_DOCKER", false)?,
            docker_image: maybe_env("DOCSRS_LOCAL_DOCKER_IMAGE")?
                .or(maybe_env("DOCSRS_DOCKER_IMAGE")?),

            build_cpu_limit: maybe_env("DOCSRS_BUILD_CPU_LIMIT")?,
            build_cpu_cores: maybe_env("DOCSRS_BUILD_CPU_CORES")?,
            include_default_targets: env("DOCSRS_INCLUDE_DEFAULT_TARGETS", true)?,
            disable_memory_limit: env("DOCSRS_DISABLE_MEMORY_LIMIT", false)?,
            build_workspace_reinitialization_interval: Duration::from_secs(env(
                "DOCSRS_BUILD_WORKSPACE_REINITIALIZATION_INTERVAL",
                86400,
            )?),
            compiler_metrics_collection_path: maybe_env("DOCSRS_COMPILER_METRICS_PATH")?,
            build_limits: Arc::new(docs_rs_build_limits::Config::from_environment()?),
        };

        if config.build_cpu_limit.is_some() && config.build_cpu_cores.is_some() {
            bail!("you only can define one of build_cpu_limit and build_cpu_cores");
        }

        Ok(config)
    }

    #[cfg(test)]
    fn test_config() -> Result<Self> {
        let mut config = Self::from_environment()?;

        config.include_default_targets = true;

        Ok(config)
    }
}

impl Config {
    /// The cargo job-limit we should set in builds.
    ///
    /// If we set either of the two CPU-limits, cargo should
    /// limit itself automatically.
    pub fn cargo_job_limit(&self) -> Option<usize> {
        self.build_cpu_cores
            .as_ref()
            .map(|c| c.len())
            .or(self.build_cpu_limit.map(|l| l as usize))
    }
}

#[derive(Debug)]
pub struct BuildCores(pub RangeInclusive<usize>);

impl BuildCores {
    pub fn len(&self) -> usize {
        self.0.size_hint().0
    }
}

impl Deref for BuildCores {
    type Target = RangeInclusive<usize>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Error)]
pub enum ParseBuildCoresError {
    #[error("expected build core range in the form <start>-<end>")]
    MissingSeparator,
    #[error("invalid build core range start `{value}`: {source}")]
    InvalidStart {
        value: String,
        #[source]
        source: ParseIntError,
    },
    #[error("invalid build core range end `{value}`: {source}")]
    InvalidEnd {
        value: String,
        #[source]
        source: ParseIntError,
    },
    #[error("build core range start must be less than or equal to end")]
    DescendingRange,
    #[error("not enough cores, we only have {0}")]
    NotEnoughCores(usize),
}

impl FromStr for BuildCores {
    type Err = ParseBuildCoresError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let (start, end) = s
            .split_once('-')
            .ok_or(ParseBuildCoresError::MissingSeparator)?;

        let start = start
            .parse()
            .map_err(|source| ParseBuildCoresError::InvalidStart {
                value: start.to_string(),
                source,
            })?;

        let end = end
            .parse()
            .map_err(|source| ParseBuildCoresError::InvalidEnd {
                value: end.to_string(),
                source,
            })?;

        if start > end {
            return Err(ParseBuildCoresError::DescendingRange);
        }

        let cpus = num_cpus::get();

        if end >= cpus {
            // NOTE: docker counts the cores zero-based, so
            // a core-number that is exactly the cpu-count is already
            // too high.
            return Err(ParseBuildCoresError::NotEnoughCores(cpus));
        }

        Ok(Self(start..=end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_build_core_range() {
        let build_cores: BuildCores = "2-3".parse().unwrap();

        assert_eq!(build_cores.start(), &2);
        assert_eq!(build_cores.end(), &3);
        assert_eq!(build_cores.len(), 2);
    }

    #[test]
    fn parses_single_build_core() {
        let build_cores: BuildCores = "2-2".parse().unwrap();

        assert_eq!(build_cores.start(), &2);
        assert_eq!(build_cores.end(), &2);
        assert_eq!(build_cores.len(), 1);
    }

    #[test]
    fn rejects_build_core_range_without_separator() {
        let err = "3".parse::<BuildCores>().unwrap_err();

        assert!(
            err.to_string()
                .contains("expected build core range in the form <start>-<end>")
        );
    }

    #[test]
    fn rejects_build_core_range_with_descending_values() {
        let err = "4-3".parse::<BuildCores>().unwrap_err();

        assert!(
            err.to_string()
                .contains("build core range start must be less than or equal to end")
        );
    }

    #[test]
    fn rejects_build_core_range_with_invalid_end() {
        let err = "3-a".parse::<BuildCores>().unwrap_err();

        assert!(err.to_string().contains("invalid build core range end `a`"));
    }

    #[test]
    fn rejects_build_core_range_with_invalid_core_number() {
        let cpus = num_cpus::get();
        let err = format!("0-{cpus}").parse::<BuildCores>().unwrap_err();

        assert!(
            err.to_string()
                .contains(&format!("not enough cores, we only have {cpus}"))
        );
    }

    #[test]
    fn cargo_jobs_uses_core_range_length() {
        let config = config_with_cpu_settings(Some(12), Some(BuildCores(3..=4)));

        assert_eq!(config.cargo_job_limit(), Some(2));
    }

    #[test]
    fn cargo_jobs_falls_back_to_cpu_limit() {
        let config = config_with_cpu_settings(Some(12), None);

        assert_eq!(config.cargo_job_limit(), Some(12));
    }

    #[test]
    fn cargo_jobs_is_none_without_cpu_settings() {
        let config = config_with_cpu_settings(None, None);

        assert_eq!(config.cargo_job_limit(), None);
    }

    fn config_with_cpu_settings(
        build_cpu_limit: Option<u32>,
        build_cpu_cores: Option<BuildCores>,
    ) -> Config {
        Config {
            prefix: PathBuf::new(),
            temp_dir: PathBuf::new(),
            compiler_metrics_collection_path: None,
            build_workspace_reinitialization_interval: Duration::from_secs(0),
            rustwide_workspace: PathBuf::new(),
            inside_docker: false,
            docker_image: None,
            build_cpu_limit,
            build_cpu_cores,
            include_default_targets: true,
            disable_memory_limit: false,
            build_limits: Arc::new(docs_rs_build_limits::Config::default()),
        }
    }
}
