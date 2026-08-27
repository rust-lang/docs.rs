use docs_rs_build_limits::Limits;
use rustwide::{
    Build, BuildDirectory, BuildResult, Crate, Toolchain, Workspace,
    cmd::{Command, DockerRuntime, SandboxBuilder},
};
use std::ops::RangeInclusive;

use crate::{ReleaseBuildResult, ReleaseContext, ReleaseOptions};

/// CPU restriction applied to the build sandbox.
#[derive(Clone, Debug, PartialEq)]
pub enum CpuLimit {
    /// Restrict the container to a fraction or number of CPU cores.
    Quota(f32),
    /// Pin the container to an inclusive range of host CPU IDs.
    Cores(RangeInclusive<usize>),
}

impl CpuLimit {
    /// Number of Cargo jobs matching this CPU restriction, when it is integral.
    pub fn cargo_jobs(&self) -> Option<usize> {
        match self {
            Self::Quota(limit) if limit.fract() == 0.0 && *limit >= 1.0 => Some(*limit as usize),
            Self::Cores(cores) => Some(cores.clone().count()),
            Self::Quota(_) => None,
        }
    }
}

/// Shared rustwide configuration used to build crate releases.
pub struct BuildEnvironment<'a> {
    workspace: &'a Workspace,
    toolchain: &'a Toolchain,
    cpu_limit: Option<CpuLimit>,
    docker_runtime: DockerRuntime,
}

impl<'a> BuildEnvironment<'a> {
    /// Create an environment using a prepared rustwide workspace and toolchain.
    pub fn new(workspace: &'a Workspace, toolchain: &'a Toolchain) -> Self {
        Self {
            workspace,
            toolchain,
            cpu_limit: None,
            docker_runtime: DockerRuntime::default(),
        }
    }

    /// Apply a CPU restriction to release sandboxes.
    pub fn cpu_limit(mut self, cpu_limit: Option<CpuLimit>) -> Self {
        self.cpu_limit = cpu_limit;
        self
    }

    /// Select the Docker runtime used for release sandboxes.
    pub fn docker_runtime(mut self, docker_runtime: DockerRuntime) -> Self {
        self.docker_runtime = docker_runtime;
        self
    }

    /// Create the sandbox builder for a release.
    pub fn sandbox(&self, limits: &Limits) -> SandboxBuilder {
        let builder = SandboxBuilder::new()
            .memory_limit(Some(limits.memory()))
            .enable_networking(limits.networking())
            .docker_runtime(self.docker_runtime);

        match &self.cpu_limit {
            Some(CpuLimit::Quota(limit)) => builder.cpu_limit(Some(*limit)),
            Some(CpuLimit::Cores(cores)) => builder.cpuset_cpus(Some(cores.clone())),
            None => builder,
        }
    }

    /// Bind prepared crate state to an active rustwide build.
    ///
    /// Metadata is loaded after rustwide prepares the source directory, then
    /// stored here once for all target and output-mode invocations.
    pub fn release<'build, 'ws>(
        &'build self,
        build: &'build Build<'ws>,
        limits: Limits,
        options: ReleaseOptions,
    ) -> anyhow::Result<ReleaseContext<'build, 'a, 'ws>> {
        ReleaseContext::new(self, build, limits, options)
    }

    /// Build all documentation artifacts for a crate release in one sandbox.
    pub fn build_release(
        &self,
        build_dir: &mut BuildDirectory,
        krate: &Crate,
        limits: Limits,
        options: ReleaseOptions,
    ) -> anyhow::Result<BuildResult<ReleaseBuildResult>> {
        let sandbox = self.sandbox(&limits);
        build_dir
            .build(self.toolchain, krate, sandbox)
            .run(|build| self.release(build, limits, options)?.build_all_targets())
    }

    pub(crate) fn workspace(&self) -> &Workspace {
        self.workspace
    }

    pub(crate) fn toolchain(&self) -> &Toolchain {
        self.toolchain
    }

    pub(crate) fn cargo_jobs(&self) -> Option<usize> {
        self.cpu_limit.as_ref().and_then(CpuLimit::cargo_jobs)
    }

    pub(crate) fn resource_suffix(&self) -> anyhow::Result<String> {
        let output = Command::new(self.workspace, self.toolchain.rustc())
            .arg("--version")
            .log_output(false)
            .run_capture()?;
        let [version] = output.stdout_lines() else {
            anyhow::bail!("invalid output returned by `rustc --version`");
        };
        Ok(format!("-{}", parse_rustc_version(version)?))
    }
}

fn parse_rustc_version(version: &str) -> anyhow::Result<String> {
    let mut outer = version.splitn(3, ' ');
    let _binary = outer.next();
    let release = outer
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing release in rustc version `{version}`"))?;
    let details = outer
        .next()
        .and_then(|details| details.strip_prefix('('))
        .and_then(|details| details.strip_suffix(')'))
        .ok_or_else(|| anyhow::anyhow!("missing details in rustc version `{version}`"))?;
    let mut details = details.split_whitespace();
    let commit = details
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing commit in rustc version `{version}`"))?;
    let date = details
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing date in rustc version `{version}`"))?;

    Ok(format!("{}-{release}-{commit}", date.replace('-', "")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_cargo_jobs_from_cpu_restrictions() {
        assert_eq!(CpuLimit::Quota(2.0).cargo_jobs(), Some(2));
        assert_eq!(CpuLimit::Quota(0.5).cargo_jobs(), None);
        assert_eq!(CpuLimit::Cores(3..=5).cargo_jobs(), Some(3));
    }

    #[test]
    fn parses_rustc_resource_version() {
        assert_eq!(
            parse_rustc_version("rustc 1.10.0-nightly (57ef01513 2016-05-23)").unwrap(),
            "20160523-1.10.0-nightly-57ef01513"
        );
    }
}
