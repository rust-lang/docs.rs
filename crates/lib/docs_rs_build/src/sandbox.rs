use docs_rs_build_limits::Limits;
use rustwide::{
    Build, Toolchain, Workspace,
    cmd::{DockerRuntime, SandboxBuilder},
};
use std::ops::RangeInclusive;

use crate::ReleaseContext;

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
    ) -> anyhow::Result<ReleaseContext<'build, 'a, 'ws>> {
        ReleaseContext::new(self, build, limits)
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
}
