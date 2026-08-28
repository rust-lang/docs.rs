use crate::{CpuLimit, ReleaseContext, StepResult};
use anyhow::{Context as _, Result, anyhow, bail};
use bon::bon;
use docs_rs_build_limits::Limits;
use docs_rs_utils::APP_USER_AGENT;
use rustwide::{
    BuildResult, Crate, Toolchain, Workspace, WorkspaceBuilder,
    cmd::{Command, CommandError, DockerRuntime, SandboxBuilder, SandboxImage},
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const DUMMY_CRATE_NAME: &str = "empty-library";
const DUMMY_CRATE_VERSION: &str = "1.0.0";
const DEFAULT_WORKSPACE_REINITIALIZATION_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Describes how the sandbox image should be resolved whenever the workspace is initialized.
#[derive(Clone, Debug, Default)]
pub enum SandboxImageSource {
    #[default]
    RustwideDefault,
    /// Require an image that is already present locally.
    Local(String),
    /// Pull the image from its registry, even if an older version is present locally.
    Remote(String),
    /// Prefer an existing local image and pull it only when it is missing.
    LocalOrRemote(String),
}

impl SandboxImageSource {
    fn resolve(&self) -> Result<Option<SandboxImage>> {
        let image = match self {
            Self::RustwideDefault => return Ok(None),
            Self::Local(name) => SandboxImage::local(name)?,
            Self::Remote(name) => SandboxImage::remote(name)?,
            Self::LocalOrRemote(name) => match SandboxImage::local(name) {
                Ok(image) => image,
                Err(CommandError::SandboxImageMissing(_)) => SandboxImage::remote(name)?,
                Err(error) => return Err(error.into()),
            },
        };
        Ok(Some(image))
    }
}

#[derive(Clone, Debug)]
struct WorkspaceConfiguration {
    path: PathBuf,
    running_inside_docker: bool,
    sandbox_image: SandboxImageSource,
    fast_init: bool,
}

impl WorkspaceConfiguration {
    fn initialize(&self) -> Result<Workspace> {
        let mut builder = WorkspaceBuilder::new(&self.path, APP_USER_AGENT)
            .running_inside_docker(self.running_inside_docker)
            .fast_init(self.fast_init);

        if let Some(image) = self.sandbox_image.resolve()? {
            builder = builder.sandbox_image(image);
        }

        let workspace = builder.init()?;
        workspace.purge_all_build_dirs()?;
        Ok(workspace)
    }
}

/// Shared rustwide workspace and toolchain configuration for docs.rs builds.
pub struct BuildEnvironment {
    workspace: Workspace,
    workspace_configuration: WorkspaceConfiguration,
    workspace_initialized_at: Instant,
    workspace_reinitialization_interval: Duration,
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
        #[builder(default)] sandbox_image: SandboxImageSource,
        #[builder(default = false)] fast_init: bool,
        #[builder(default = DEFAULT_WORKSPACE_REINITIALIZATION_INTERVAL)]
        workspace_reinitialization_interval: Duration,
        cpu_limit: Option<CpuLimit>,
        #[builder(default)] docker_runtime: DockerRuntime,
        #[builder(default = false)] include_default_targets: bool,
        #[builder(default)] default_limits: Limits,
    ) -> Result<Self> {
        let workspace_configuration = WorkspaceConfiguration {
            path: path.to_owned(),
            running_inside_docker,
            sandbox_image,
            fast_init,
        };
        let workspace = workspace_configuration.initialize()?;

        Ok(Self {
            workspace,
            workspace_configuration,
            workspace_initialized_at: Instant::now(),
            workspace_reinitialization_interval,
            toolchain,
            cpu_limit,
            docker_runtime,
            include_default_targets,
            default_limits,
        })
    }

    /// Recreate the workspace when its configured refresh interval has elapsed.
    ///
    /// Resolving a [`SandboxImageSource::Remote`] pulls the tag again, allowing
    /// long-running builders to pick up a newly published sandbox image.
    pub fn refresh_workspace_if_interval_passed(&mut self) -> Result<bool> {
        if self.workspace_initialized_at.elapsed() < self.workspace_reinitialization_interval {
            return Ok(false);
        }

        self.workspace = self.workspace_configuration.initialize()?;
        self.workspace_initialized_at = Instant::now();
        Ok(true)
    }

    /// Remove all cached registry, Git, and toolchain data from the workspace.
    pub fn purge_caches(&self) -> Result<()> {
        self.workspace.purge_all_caches()?;
        Ok(())
    }

    /// Remove all build directories from the workspace.
    pub fn purge_build_directories(&self) -> Result<()> {
        self.workspace.purge_all_build_dirs()?;
        Ok(())
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
        Ok(format!("-{}", parse_rustc_version(&self.rustc_version()?)?))
    }

    pub(crate) fn rustc_version(&self) -> Result<String> {
        if let Some(ci) = self.toolchain.as_ci() {
            return Ok(ci_rustc_version(ci.sha()));
        }

        let output = Command::new(self.workspace(), self.toolchain.rustc())
            .arg("--version")
            .log_output(false)
            .run_capture()?;
        let [version] = output.stdout_lines() else {
            bail!("invalid output returned by `rustc --version`");
        };
        Ok(version.clone())
    }

    pub(crate) fn ensure_target_installed(&self, target: impl AsRef<str>) -> Result<()> {
        let target = target.as_ref();
        self.configured_toolchain()
            .add_target(self.workspace(), target)
            .context("error adding non-default target to toolchain")?;

        Ok(())
    }
}

fn ci_rustc_version(sha: &str) -> String {
    format!("rustc 1.9999.0-nightly ({sha} 2999-12-29)")
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

    #[test]
    fn creates_ci_rustc_resource_version() {
        assert_eq!(
            parse_rustc_version(&ci_rustc_version("0123456789abcdef")).unwrap(),
            "29991229-1.9999.0-nightly-0123456789abcdef"
        );
    }
}
