use anyhow::Context as _;
use docs_rs_build_limits::Limits;
use rustwide::{
    BuildResult, Crate, Toolchain, Workspace,
    cmd::{Command, DockerRuntime, SandboxBuilder},
};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    ops::RangeInclusive,
};
use tracing::warn;

use crate::{ActiveReleaseBuild, ReleaseBuildResult, StepResult};
use std::path::PathBuf;

const DUMMY_CRATE_NAME: &str = "empty-library";
const DUMMY_CRATE_VERSION: &str = "1.0.0";

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

/// A crate release whose build lifecycle is managed by docs.rs.
pub struct ReleaseContext<'release, 'env> {
    environment: &'release BuildEnvironment<'env>,
    krate: &'release Crate,
    limits: Limits,
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

    /// Create a release whose build directory, sandbox, and caches are managed
    /// by this library.
    pub fn release<'release>(
        &'release self,
        krate: &'release Crate,
        limits: Limits,
    ) -> ReleaseContext<'release, 'a> {
        ReleaseContext {
            environment: self,
            krate,
            limits,
        }
    }

    /// Build the shared rustdoc static files for this toolchain.
    ///
    /// Essential files are toolchain-wide and are always built from docs.rs's
    /// dummy `empty-library` crate for the host target.
    pub fn build_essential_files(
        &self,
        limits: Limits,
    ) -> anyhow::Result<BuildResult<StepResult<PathBuf>>> {
        let krate = Crate::crates_io(DUMMY_CRATE_NAME, DUMMY_CRATE_VERSION);
        self.release(&krate, limits)
            .run(|build| Ok(build.build_essential_files()))
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

impl<'release, 'env> ReleaseContext<'release, 'env> {
    /// Build coverage, rustdoc JSON, and HTML for the full docs.rs target set.
    pub fn build_all_targets(self) -> anyhow::Result<BuildResult<ReleaseBuildResult>> {
        self.run(|build| build.build_all_targets())
    }

    /// Build only targets explicitly selected by the crate's docs.rs metadata.
    pub fn build_configured_targets(self) -> anyhow::Result<BuildResult<ReleaseBuildResult>> {
        self.run(|build| build.build_configured_targets())
    }

    /// Run selected build operations in one reusable sandbox.
    ///
    /// Fetching, build-directory creation, and cache cleanup are handled around
    /// the callback. The active build exposes the artifact-specific methods.
    pub fn run<R>(
        self,
        callback: impl for<'build, 'ws> FnOnce(
            ActiveReleaseBuild<'build, 'env, 'ws>,
        ) -> anyhow::Result<R>,
    ) -> anyhow::Result<BuildResult<R>> {
        let Self {
            environment,
            krate,
            limits,
        } = self;

        environment.workspace.purge_all_build_dirs()?;
        krate.fetch(environment.workspace)?;

        let mut build_dir = environment.workspace.build_dir(&build_dir_name(krate));
        let sandbox = environment.sandbox(&limits);
        let result = build_dir
            .build(environment.toolchain, krate, sandbox)
            .run(|build| callback(ActiveReleaseBuild::new(environment, build, limits)?));

        finish_cached_build(environment.workspace, krate, result)
    }
}

fn build_dir_name(krate: &Crate) -> String {
    let mut hasher = DefaultHasher::new();
    krate.to_string().hash(&mut hasher);
    format!("release-{:016x}", hasher.finish())
}

fn finish_cached_build<T>(
    workspace: &Workspace,
    krate: &Crate,
    result: anyhow::Result<T>,
) -> anyhow::Result<T> {
    let purge = krate
        .purge_from_cache(workspace)
        .context("purging the crate from rustwide's cache");

    match (result, purge) {
        (Ok(output), Ok(())) => Ok(output),
        (Ok(_), Err(purge_error)) => Err(purge_error),
        (Err(build_error), Ok(())) => Err(build_error),
        (Err(build_error), Err(purge_error)) => {
            warn!(?purge_error, "failed to purge crate after failed build");
            Err(build_error)
        }
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
