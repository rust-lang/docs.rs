use crate::{CpuLimit, ReleaseContext, StepResult};
use anyhow::{Context as _, Result, anyhow, bail};
use bon::bon;
use docs_rs_build_limits::Limits;
use docsrs_metadata::DEFAULT_TARGETS;
use rustwide::{
    BuildResult, Crate, Toolchain, Workspace, WorkspaceBuilder,
    cmd::{Command, CommandError, DockerRuntime, SandboxBuilder, SandboxImage},
};
use std::path::{Path, PathBuf};

const DUMMY_CRATE_NAME: &str = "empty-library";
const DUMMY_CRATE_VERSION: &str = "1.0.0";

/// User agent used when the docs.rs build environment accesses remote services.
pub const DOCS_RS_USER_AGENT: &str = "docs.rs builder (https://github.com/rust-lang/docs.rs)";

/// Resolve a sandbox image name, preferring an existing local image and
/// falling back to a remote image that rustwide will pull when needed.
pub fn resolve_sandbox_image(name: &str) -> Result<SandboxImage> {
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
    // default limits on the builder host.
    default_limits: Limits,
}

#[bon]
impl BuildEnvironment {
    #[builder(
        on(_, into),
        finish_fn(name = build),
    )]
    pub fn builder(
        #[builder(start_fn)] path: &Path,
        #[builder(default = Toolchain::dist("nightly"))] toolchain: Toolchain,
        #[builder(default = false)] running_inside_docker: bool,
        mut sandbox_image: Option<SandboxImage>,
        #[builder(default = false)] fast_init: bool,
        cpu_limit: Option<CpuLimit>,
        #[builder(default)] docker_runtime: DockerRuntime,
        #[builder(default = false)] include_default_targets: bool,
        #[builder(default)] default_limits: Limits,
    ) -> Result<Self> {
        let mut builder = WorkspaceBuilder::new(path, DOCS_RS_USER_AGENT)
            .running_inside_docker(running_inside_docker)
            .fast_init(fast_init);

        if let Some(image) = sandbox_image.take() {
            builder = builder.sandbox_image(image);
        }

        Ok(Self {
            workspace: builder.init()?,
            toolchain,
            cpu_limit,
            docker_runtime,
            include_default_targets,
            default_limits,
        })
    }

    /// Enter the context of a single release.
    pub fn release<'release>(&'release self, krate: &'release Crate) -> ReleaseContext<'release> {
        ReleaseContext {
            environment: self,
            krate,
            limits: None,
        }
    }

    /// Build the shared rustdoc static files for this toolchain.
    pub fn build_essential_files(&self) -> Result<BuildResult<StepResult<PathBuf>>> {
        let krate = Crate::crates_io(DUMMY_CRATE_NAME, DUMMY_CRATE_VERSION);
        self.release(&krate)
            .run(|build| Ok(build.build_essential_files()))
    }

    pub(crate) fn sandbox_builder(&self, limits: &Limits) -> SandboxBuilder {
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

    pub(crate) fn configured_toolchain(&self) -> &Toolchain {
        &self.toolchain
    }

    pub(crate) fn cargo_jobs(&self) -> Option<usize> {
        self.cpu_limit.as_ref().and_then(CpuLimit::cargo_jobs)
    }

    pub(crate) fn includes_default_targets(&self) -> bool {
        self.include_default_targets
    }

    pub(crate) fn default_limits(&self) -> &Limits {
        &self.default_limits
    }

    pub(crate) fn resource_suffix(&self) -> Result<String> {
        let output = Command::new(self.workspace(), self.toolchain.rustc())
            .arg("--version")
            .log_output(false)
            .run_capture()?;
        let [version] = output.stdout_lines() else {
            bail!("invalid output returned by `rustc --version`");
        };
        Ok(format!("-{}", parse_rustc_version(version)?))
    }

    pub(crate) fn add_target(&self, target: impl AsRef<str>) -> Result<()> {
        let target = target.as_ref();
        if !DEFAULT_TARGETS.contains(&target) {
            self.configured_toolchain()
                .add_target(self.workspace(), target)
                .context("error adding non-default target to toolchain")?;
        }
        Ok(())
    }
}

fn parse_rustc_version(version: &str) -> Result<String> {
    let mut outer = version.splitn(3, ' ');
    let _binary = outer.next();
    let release = outer
        .next()
        .ok_or_else(|| anyhow!("missing release in rustc version `{version}`"))?;
    let details = outer
        .next()
        .and_then(|value| value.strip_prefix('('))
        .and_then(|value| value.strip_suffix(')'))
        .ok_or_else(|| anyhow!("missing details in rustc version `{version}`"))?;
    let mut details = details.split_whitespace();
    let commit = details
        .next()
        .ok_or_else(|| anyhow!("missing commit in rustc version `{version}`"))?;
    let date = details
        .next()
        .ok_or_else(|| anyhow!("missing date in rustc version `{version}`"))?;
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
