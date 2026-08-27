use crate::{CpuLimit, ReleaseContext, StepResult};
use anyhow::Result;
use docs_rs_build_limits::Limits;
use rustwide::{
    BuildResult, Crate, Toolchain, Workspace, WorkspaceBuilder,
    cmd::{Command, CommandError, DockerRuntime, SandboxBuilder, SandboxImage},
};
use std::path::PathBuf;

const DUMMY_CRATE_NAME: &str = "empty-library";
const DUMMY_CRATE_VERSION: &str = "1.0.0";

/// User agent used when the docs.rs build environment accesses remote services.
pub const DOCS_RS_USER_AGENT: &str = "docs.rs builder (https://github.com/rust-lang/docs.rs)";

/// Configuration for the rustwide workspace used by a docs.rs build.
pub struct WorkspaceConfig {
    /// Persistent directory containing rustwide state and caches.
    pub path: PathBuf,
    /// Whether the build driver itself is running in a container.
    pub running_inside_docker: bool,
    /// Optional override for rustwide's default sandbox image.
    pub sandbox_image: Option<String>,
    /// Prefer initialization speed over runtime performance.
    pub fast_init: bool,
    toolchain: Toolchain,
}

impl WorkspaceConfig {
    /// Create a workspace configuration using rustwide's default sandbox image.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            running_inside_docker: false,
            sandbox_image: None,
            fast_init: false,
            toolchain: Toolchain::dist("nightly"),
        }
    }

    /// Override the nightly toolchain used by default.
    pub fn toolchain(mut self, toolchain: Toolchain) -> Self {
        self.toolchain = toolchain;
        self
    }

    /// Initialize the configured rustwide workspace.
    pub fn init(self) -> Result<BuildEnvironment> {
        let mut builder = WorkspaceBuilder::new(&self.path, DOCS_RS_USER_AGENT)
            .running_inside_docker(self.running_inside_docker)
            .fast_init(self.fast_init);

        if let Some(image_name) = &self.sandbox_image {
            let image = resolve_image(image_name)?;
            builder = builder.sandbox_image(image);
        }

        Ok(BuildEnvironment::new(builder.init()?, self.toolchain))
    }
}

fn resolve_image(name: &str) -> Result<SandboxImage> {
    match SandboxImage::local(name) {
        Ok(image) => Ok(image),
        Err(CommandError::SandboxImageMissing(_)) => Ok(SandboxImage::remote(name)?),
        Err(error) => Err(error.into()),
    }
}

/// Shared rustwide workspace and toolchain configuration for docs.rs builds.
pub struct BuildEnvironment {
    workspace: Workspace,
    toolchain: Toolchain,
    cpu_limit: Option<CpuLimit>,
    docker_runtime: DockerRuntime,
    include_default_targets: bool,
}

impl BuildEnvironment {
    /// Create an environment using a prepared rustwide workspace and toolchain.
    pub fn new(workspace: Workspace, toolchain: Toolchain) -> Self {
        Self {
            workspace,
            toolchain,
            cpu_limit: None,
            docker_runtime: DockerRuntime::default(),
            include_default_targets: true,
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

    /// Configure whether crates without an explicit target list are built for
    /// docs.rs's standard target set in addition to their default target.
    pub fn include_default_targets(mut self, include: bool) -> Self {
        self.include_default_targets = include;
        self
    }

    /// Create a release managed by this environment.
    pub fn release<'release>(
        &'release self,
        krate: &'release Crate,
        limits: Limits,
    ) -> ReleaseContext<'release> {
        ReleaseContext {
            environment: self,
            krate,
            limits,
        }
    }

    /// Build the shared rustdoc static files for this toolchain.
    pub fn build_essential_files(
        &self,
        limits: Limits,
    ) -> anyhow::Result<BuildResult<StepResult<PathBuf>>> {
        let krate = Crate::crates_io(DUMMY_CRATE_NAME, DUMMY_CRATE_VERSION);
        self.release(&krate, limits)
            .run(|build| Ok(build.build_essential_files()))
    }

    pub(crate) fn sandbox(&self, limits: &Limits) -> SandboxBuilder {
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

    pub(crate) fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub(crate) fn toolchain(&self) -> &Toolchain {
        &self.toolchain
    }

    pub(crate) fn cargo_jobs(&self) -> Option<usize> {
        self.cpu_limit.as_ref().and_then(CpuLimit::cargo_jobs)
    }

    pub(crate) fn includes_default_targets(&self) -> bool {
        self.include_default_targets
    }

    pub(crate) fn resource_suffix(&self) -> anyhow::Result<String> {
        let output = Command::new(&self.workspace, self.toolchain.rustc())
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
        .and_then(|value| value.strip_prefix('('))
        .and_then(|value| value.strip_suffix(')'))
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
    fn parses_rustc_resource_version() {
        assert_eq!(
            parse_rustc_version("rustc 1.10.0-nightly (57ef01513 2016-05-23)").unwrap(),
            "20160523-1.10.0-nightly-57ef01513"
        );
    }
}
