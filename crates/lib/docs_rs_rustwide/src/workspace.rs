use crate::{
    BuildResult, CpuLimit, HtmlOutput, ReleaseContext, StepResultExt, ToolchainExt as _,
    toolchain::ManagedToolchain, workspace_lock::WorkspaceLock,
};
use anyhow::{Result, bail};
use bon::bon;
use docs_rs_build_limits::Limits;
use docs_rs_types::ByteSize;
use docs_rs_utils::{APP_USER_AGENT, retry};
use rustwide::{
    Crate, Toolchain, Workspace, WorkspaceBuilder,
    cmd::{CommandError, DockerRuntime, SandboxBuilder, SandboxImage},
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tracing::{debug, instrument};

const DUMMY_CRATE_NAME: &str = "empty-library";
const DUMMY_CRATE_VERSION: &str = "1.0.0";

pub const SANDBOX_IMAGE_LINUX: &str = "ghcr.io/rust-lang/crates-build-env/linux";
pub const SANDBOX_IMAGE_LINUX_MICRO: &str = "ghcr.io/rust-lang/crates-build-env/linux-micro";

/// Controls whether a named sandbox image may be pulled from its registry.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, strum::Display, strum::EnumString)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
#[strum(serialize_all = "kebab-case")]
pub enum ImagePullPolicy {
    /// Require an image that is already present locally.
    Local,
    /// Pull the image even if an older version is present locally.
    Remote,
    /// Prefer an existing local image and pull only when it is missing.
    #[default]
    LocalOrRemote,
}

/// Describes how the sandbox image should be resolved whenever the workspace is initialized.
#[derive(Clone, Debug, Default)]
pub enum SandboxImageSource {
    /// Use Rustwide's default image and resolution behavior.
    #[default]
    RustwideDefault,
    /// Resolve a named image using the selected pull policy.
    Image {
        name: String,
        source: ImagePullPolicy,
    },
}

impl SandboxImageSource {
    #[instrument(skip_all)]
    fn resolve(&self) -> Result<Option<SandboxImage>> {
        match self {
            SandboxImageSource::RustwideDefault => Ok(None),
            SandboxImageSource::Image { name, source } => Ok(Some(match source {
                ImagePullPolicy::Local => SandboxImage::local(name)?,
                ImagePullPolicy::Remote => SandboxImage::remote(name)?,
                ImagePullPolicy::LocalOrRemote => match SandboxImage::local(name) {
                    Ok(image) => image,
                    Err(CommandError::SandboxImageMissing(_)) => SandboxImage::remote(name)?,
                    Err(error) => return Err(error.into()),
                },
            })),
        }
    }

    pub fn local(name: impl Into<String>) -> Self {
        Self::Image {
            name: name.into(),
            source: ImagePullPolicy::Local,
        }
    }

    pub fn remote(name: impl Into<String>) -> Self {
        Self::Image {
            name: name.into(),
            source: ImagePullPolicy::Remote,
        }
    }

    pub fn local_or_remote(name: impl Into<String>) -> Self {
        Self::Image {
            name: name.into(),
            source: ImagePullPolicy::LocalOrRemote,
        }
    }
}

#[derive(Clone, Debug)]
struct WorkspaceConfiguration {
    path: PathBuf,
    running_inside_docker: bool,
    sandbox_image: SandboxImageSource,
    fast_init: bool,
    reinitialization_interval: Option<Duration>,
}

impl WorkspaceConfiguration {
    #[instrument(skip_all)]
    fn create_workspace(&self) -> Result<Workspace> {
        debug!(
            path = %self.path.display(),
            running_inside_docker = self.running_inside_docker,
            fast_init = self.fast_init,
            sandbox_image = ?self.sandbox_image,
            "initializing rustwide workspace"
        );

        let mut builder = WorkspaceBuilder::new(&self.path, APP_USER_AGENT)
            .running_inside_docker(self.running_inside_docker)
            .fast_init(self.fast_init);

        if let Some(image) = self.sandbox_image.resolve()? {
            builder = builder.sandbox_image(image);
        }

        let workspace = builder.init()?;

        retry(|| workspace.purge_all_build_dirs(), 3)?;
        debug!("rustwide workspace initialized");
        Ok(workspace)
    }
}

struct ManagedWorkspace {
    workspace: Workspace,
    configuration: WorkspaceConfiguration,
    initialized_at: Instant,
}

impl ManagedWorkspace {
    #[instrument(skip_all)]
    fn new(configuration: WorkspaceConfiguration) -> Result<Self> {
        let workspace = configuration.create_workspace()?;
        debug!(
            path = %configuration.path.display(),
            "creating Managed Workspace"
        );
        Ok(Self {
            workspace,
            configuration,
            initialized_at: Instant::now(),
        })
    }

    #[instrument(skip_all)]
    fn refresh_if_due(&mut self) -> Result<bool> {
        let Some(reinitialization_interval) = self.configuration.reinitialization_interval else {
            return Ok(false);
        };

        let elapsed = self.initialized_at.elapsed();
        if elapsed < reinitialization_interval {
            debug!(?elapsed, "workspace refresh is not due");
            return Ok(false);
        }

        debug!(?elapsed, "refreshing rustwide workspace");
        self.workspace = self.configuration.create_workspace()?;
        self.initialized_at = Instant::now();
        debug!("rustwide workspace refreshed");
        Ok(true)
    }

    fn get(&self) -> &Workspace {
        &self.workspace
    }
}

/// Changes made by [`BuildEnvironment::perform_maintenance`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct MaintenanceResult {
    /// Whether the rustwide workspace was recreated.
    pub workspace_refreshed: bool,
    /// Whether the configured compiler changed.
    pub toolchain_updated: bool,
}

/// Shared rustwide workspace and toolchain configuration for docs.rs builds.
///
/// Holds an exclusive filesystem lock until dropped, including during maintenance.
/// Initialization fails if the workspace is already in use unless
/// `wait_for_workspace_lock(true)` is selected. Keep this environment alive until
/// its build artifacts have been consumed or copied out of the workspace.
pub struct BuildEnvironment {
    workspace: ManagedWorkspace,
    toolchain: ManagedToolchain,
    cpu_limit: Option<CpuLimit>,
    docker_runtime: DockerRuntime,
    include_default_targets: bool,
    validate_host_resources: bool,
    compiler_metrics_collection_path: Option<PathBuf>,
    // default limits on the builder host.
    default_limits: Limits,
    // Drop last: workspace resources must be released before another owner enters.
    _lock: WorkspaceLock,
}

#[bon]
impl BuildEnvironment {
    #[builder(
        on(_, into),
        finish_fn(name = build),
    )]
    pub fn builder(
        #[builder(start_fn)] path: &Path,
        #[builder(default = Toolchain::default())] toolchain: Toolchain,
        #[builder(default = false)] running_inside_docker: bool,
        #[builder(default)] sandbox_image: SandboxImageSource,
        #[builder(default = false)] fast_init: bool,
        /// Wait for another environment to release this workspace instead of failing.
        #[builder(default = false)]
        wait_for_workspace_lock: bool,
        /// Enable periodic workspace refreshes. Omitted means automatic refresh is disabled.
        workspace_reinitialization_interval: Option<Duration>,
        /// Enable periodic toolchain updates. Omitted means automatic updates are disabled.
        toolchain_update_interval: Option<Duration>,
        cpu_limit: Option<CpuLimit>,
        #[builder(default)] docker_runtime: DockerRuntime,
        #[builder(default = false)] include_default_targets: bool,
        #[builder(default = true)] validate_host_resources: bool,
        compiler_metrics_collection_path: Option<PathBuf>,
        #[builder(default)] default_limits: Limits,
    ) -> Result<Self> {
        if !crate::logging::is_initialized() {
            bail!(
                "Rustwide logging is not initialized; call \
                 docs_rs_rustwide::logging::init(log_build_logs) \
                 before creating a BuildEnvironment"
            );
        }

        let lock = WorkspaceLock::acquire(path, wait_for_workspace_lock)?;
        let workspace_configuration = WorkspaceConfiguration {
            path: path.to_owned(),
            running_inside_docker,
            sandbox_image,
            fast_init,
            reinitialization_interval: workspace_reinitialization_interval,
        };
        let workspace = ManagedWorkspace::new(workspace_configuration)?;

        let mut environment = Self {
            workspace,
            toolchain: ManagedToolchain::new(toolchain, toolchain_update_interval),
            cpu_limit,
            docker_runtime,
            include_default_targets,
            validate_host_resources,
            compiler_metrics_collection_path,
            default_limits,
            _lock: lock,
        };
        environment.ensure_toolchain_ready()?;
        Ok(environment)
    }

    /// Perform the maintenance required before starting the next release build.
    ///
    /// The workspace is refreshed and the toolchain is checked for updates only
    /// when their independently configured intervals have elapsed. Both are disabled
    /// by default. With a toolchain update interval configured, the first maintenance
    /// call checks for an update. Explicit `update_toolchain()` calls work regardless
    /// of whether an interval is configured.
    /// Workspace initialization, toolchain installation, target changes, and cache
    /// cleanup retry their own failing operations. Callers should propagate a
    /// maintenance failure rather than retry the entire sequence, which may have
    /// already changed the workspace or toolchain.
    /// Refreshing a workspace configured with [`SandboxImageSource::remote`]
    /// resolves and pulls the image again, allowing a long-running builder to
    /// pick up newly published versions of the same remote image tag.
    ///
    /// Long-running, sequential builders should call this method regularly
    /// between release builds, typically after each completed build or before
    /// dequeuing the next one. Calling it that often is inexpensive when
    /// neither interval has elapsed. One-shot local builds may skip maintenance:
    /// constructing the environment already ensures that its configured
    /// toolchain, targets, and components are ready.
    ///
    /// When [`MaintenanceResult::toolchain_updated`] is `true`, callers that
    /// publish rustdoc's shared static files should rebuild and publish them
    /// before processing the next release.
    #[instrument(skip_all)]
    pub fn perform_maintenance(&mut self) -> Result<MaintenanceResult> {
        let workspace_refreshed = self.workspace.refresh_if_due()?;
        if workspace_refreshed {
            debug!("ensuring toolchain readiness after workspace refresh");
            self.ensure_toolchain_ready()?;
        }
        let toolchain_update_due = self.toolchain.update_due(Instant::now());
        let toolchain_updated = if toolchain_update_due {
            debug!("toolchain update check is due");
            self.update_toolchain()?
        } else {
            debug!("toolchain update check is not due");
            false
        };

        debug!(
            workspace_refreshed,
            toolchain_updated, "maintenance complete"
        );

        Ok(MaintenanceResult {
            workspace_refreshed,
            toolchain_updated,
        })
    }

    #[instrument(skip_all)]
    fn purge_caches(&mut self) -> Result<()> {
        debug!("purging rustwide caches");
        retry(|| self.workspace().purge_all_caches(), 3)?;
        debug!("rustwide caches purged");
        Ok(())
    }

    /// Select the toolchain used by subsequent builds.
    ///
    /// The selected toolchain is made ready before this method returns. The
    /// result reports whether the toolchain itself had to be installed.
    #[instrument(skip_all)]
    pub fn set_toolchain(&mut self, toolchain: Toolchain) -> Result<bool> {
        let selection_changed = self.toolchain.select(toolchain);
        let installed = self.ensure_toolchain_ready()?;
        if selection_changed && !installed {
            self.purge_caches()?;
        }
        debug!(selection_changed, installed, "toolchain selection ready");
        Ok(installed)
    }

    /// Return the toolchain currently selected for builds.
    pub fn toolchain(&self) -> &Toolchain {
        self.toolchain.get()
    }

    fn ensure_toolchain_ready(&mut self) -> Result<bool> {
        let installed = self.toolchain.ensure_ready(self.workspace())?;
        if installed {
            self.purge_caches()?;
        }
        Ok(installed)
    }

    /// Immediately install or update the configured toolchain, bypassing the
    /// update interval used by [`Self::perform_maintenance`].
    ///
    /// This also ensures the docs.rs targets and components are ready and
    /// purges incompatible workspace caches when the compiler changes. A
    /// successful call resets the maintenance interval. CI toolchains always
    /// report a change because their existing version cannot be detected
    /// through rustup reliably.
    #[instrument(skip_all)]
    pub fn update_toolchain(&mut self) -> Result<bool> {
        let changed = self.toolchain.update(self.workspace())?;
        if changed {
            self.purge_caches()?;
        }
        self.toolchain.mark_updated(Instant::now());
        Ok(changed)
    }

    /// Enter the context of a single release.
    ///
    /// The returned context retains exclusive access to this environment for
    /// its full fetch and build lifecycle. This prevents maintenance or another
    /// release from concurrently modifying the shared rustwide workspace.
    pub fn release<'release>(
        &'release mut self,
        krate: &'release Crate,
    ) -> ReleaseContext<'release> {
        ReleaseContext {
            environment: self,
            krate,
            limits: None,
            directory_label: None,
            state: crate::release::Unfetched,
        }
    }

    /// Build the shared rustdoc static files for this toolchain.
    ///
    /// Like a release build, this requires exclusive access to the shared
    /// rustwide workspace.
    #[instrument(skip_all)]
    pub fn build_essential_files(&mut self) -> Result<BuildResult<HtmlOutput>> {
        let krate = Crate::crates_io(DUMMY_CRATE_NAME, DUMMY_CRATE_VERSION);
        self.release(&krate)
            .run(|build| Ok(build.build_essential_files().into_inner()?))
    }

    pub(crate) fn sandbox_builder(&self, limits: &Limits) -> SandboxBuilder {
        let builder = SandboxBuilder::new()
            .memory_limit(Some(limits.memory().as_u64() as usize))
            .enable_networking(limits.networking())
            .docker_runtime(self.docker_runtime);
        match &self.cpu_limit {
            Some(CpuLimit::Quota(limit)) => builder.cpu_limit(Some(limit.get())),
            Some(CpuLimit::Cores(cores)) => builder.cpuset_cpus(Some(cores.get())),
            None => builder,
        }
    }

    pub(crate) fn workspace(&self) -> &Workspace {
        self.workspace.get()
    }

    pub(crate) fn configured_toolchain(&self) -> &Toolchain {
        self.toolchain.get()
    }

    pub(crate) fn cargo_jobs(&self) -> Option<usize> {
        self.cpu_limit.as_ref().and_then(CpuLimit::cargo_jobs)
    }

    pub(crate) fn includes_default_targets(&self) -> bool {
        self.include_default_targets
    }

    pub(crate) fn validate_host_resources(&self, limits: &Limits) -> Result<()> {
        if !self.validate_host_resources {
            return Ok(());
        }

        debug!("validating host resources");

        let system = sysinfo::System::new_with_specifics(
            sysinfo::RefreshKind::nothing()
                .with_memory(sysinfo::MemoryRefreshKind::nothing().with_ram()),
        );
        let available = ByteSize::b(system.available_memory());
        if limits.memory() > available {
            bail!(
                "not enough host memory for build: needed {}, have {}",
                limits.memory(),
                available,
            );
        }
        Ok(())
    }

    pub(crate) fn compiler_metrics_collection_path(&self) -> Option<&Path> {
        self.compiler_metrics_collection_path.as_deref()
    }

    pub(crate) fn default_limits(&self) -> &Limits {
        &self.default_limits
    }

    pub(crate) fn resource_suffix(&self) -> Result<String> {
        self.toolchain.resource_suffix(self.workspace())
    }

    /// Return the version reported by the configured Rust compiler.
    ///
    /// CI toolchains use a stable synthetic version because rustup's normal
    /// `+toolchain` invocation cannot address CI artifacts.
    #[instrument(skip_all)]
    pub fn rustc_version(&self) -> Result<String> {
        self.toolchain.rustc_version(self.workspace())
    }

    pub(crate) fn ensure_target_installed(&self, target: impl AsRef<str>) -> Result<()> {
        self.toolchain
            .ensure_target_installed(self.workspace(), target)
    }
}
