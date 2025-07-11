use crate::RUSTDOC_STATIC_STORAGE_PREFIX;
use crate::db::{
    BuildId,
    file::{add_path_into_database, file_list_to_json},
};
use crate::db::{CrateId, ReleaseId};
use crate::db::{
    Pool, add_doc_coverage, add_path_into_remote_archive, finish_build, finish_release,
    initialize_build, initialize_crate, initialize_release, types::BuildStatus,
    update_build_with_error, update_crate_data_in_database,
};
use crate::docbuilder::Limits;
use crate::error::Result;
use crate::repositories::RepositoryStatsUpdater;
use crate::storage::{
    CompressionAlgorithm, RustdocJsonFormatVersion, compress, get_file_list, rustdoc_archive_path,
    rustdoc_json_path, source_archive_path,
};
use crate::utils::{
    CargoMetadata, ConfigName, copy_dir_all, get_config, parse_rustc_version, report_error,
    set_config,
};
use crate::{AsyncStorage, Config, Context, InstanceMetrics, RegistryApi, Storage};
use crate::{db::blacklist::is_blacklisted, utils::MetadataPackage};
use anyhow::{Context as _, Error, anyhow, bail};
use docsrs_metadata::{BuildTargets, DEFAULT_TARGETS, HOST_TARGET, Metadata};
use itertools::Itertools as _;
use regex::Regex;
use rustwide::cmd::{Command, CommandError, SandboxBuilder, SandboxImage};
use rustwide::logging::{self, LogStorage};
use rustwide::toolchain::ToolchainError;
use rustwide::{AlternativeRegistry, Build, Crate, Toolchain, Workspace, WorkspaceBuilder};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;
use tokio::runtime::Runtime;
use tracing::{debug, error, info, info_span, instrument, warn};

const USER_AGENT: &str = "docs.rs builder (https://github.com/rust-lang/docs.rs)";
const COMPONENTS: &[&str] = &["llvm-tools-preview", "rustc-dev", "rustfmt"];
const DUMMY_CRATE_NAME: &str = "empty-library";
const DUMMY_CRATE_VERSION: &str = "1.0.0";

pub const RUSTDOC_JSON_COMPRESSION_ALGORITHMS: &[CompressionAlgorithm] =
    &[CompressionAlgorithm::Zstd, CompressionAlgorithm::Gzip];

/// read the format version from a rustdoc JSON file.
fn read_format_version_from_rustdoc_json(
    reader: impl std::io::Read,
) -> Result<RustdocJsonFormatVersion> {
    let reader = BufReader::new(reader);

    #[derive(Deserialize)]
    struct RustdocJson {
        format_version: u16,
    }

    let rustdoc_json: RustdocJson = serde_json::from_reader(reader)?;

    Ok(RustdocJsonFormatVersion::Version(
        rustdoc_json.format_version,
    ))
}

async fn get_configured_toolchain(conn: &mut sqlx::PgConnection) -> Result<Toolchain> {
    let name: String = get_config(conn, ConfigName::Toolchain)
        .await?
        .unwrap_or_else(|| "nightly".into());

    // If the toolchain is all hex, assume it references an artifact from
    // CI, for instance an `@bors try` build.
    let re = Regex::new(r"^[a-fA-F0-9]+$").unwrap();
    if re.is_match(&name) {
        debug!("using CI build {}", &name);
        Ok(Toolchain::ci(&name, false))
    } else {
        debug!("using toolchain {}", &name);
        Ok(Toolchain::dist(&name))
    }
}

fn build_workspace<C: Context>(context: &C) -> Result<Workspace> {
    let config = context.config()?;

    let mut builder = WorkspaceBuilder::new(&config.rustwide_workspace, USER_AGENT)
        .running_inside_docker(config.inside_docker);
    if let Some(custom_image) = &config.docker_image {
        let image = match SandboxImage::local(custom_image) {
            Ok(i) => i,
            Err(CommandError::SandboxImageMissing(_)) => SandboxImage::remote(custom_image)?,
            Err(err) => return Err(err.into()),
        };
        builder = builder.sandbox_image(image);
    }
    if cfg!(test) {
        builder = builder.fast_init(true);
    }

    let workspace = builder.init()?;
    workspace.purge_all_build_dirs()?;
    Ok(workspace)
}

#[derive(Debug)]
pub enum PackageKind<'a> {
    Local(&'a Path),
    CratesIo,
    Registry(&'a str),
}

pub struct RustwideBuilder {
    workspace: Workspace,
    toolchain: Toolchain,
    runtime: Arc<Runtime>,
    config: Arc<Config>,
    db: Pool,
    storage: Arc<Storage>,
    async_storage: Arc<AsyncStorage>,
    metrics: Arc<InstanceMetrics>,
    registry_api: Arc<RegistryApi>,
    repository_stats_updater: Arc<RepositoryStatsUpdater>,
    workspace_initialize_time: Instant,
}

impl RustwideBuilder {
    pub fn init<C: Context>(context: &C) -> Result<Self> {
        let config = context.config()?;
        let pool = context.pool()?;
        let runtime = context.runtime()?;
        let toolchain = runtime.block_on(async {
            let mut conn = pool.get_async().await?;
            get_configured_toolchain(&mut conn).await
        })?;

        Ok(RustwideBuilder {
            workspace: build_workspace(context)?,
            toolchain,
            config,
            db: pool,
            runtime: runtime.clone(),
            storage: context.storage()?,
            async_storage: runtime.block_on(context.async_storage())?,
            metrics: context.instance_metrics()?,
            registry_api: context.registry_api()?,
            repository_stats_updater: context.repository_stats_updater()?,
            workspace_initialize_time: Instant::now(),
        })
    }

    pub fn reinitialize_workspace_if_interval_passed<C: Context>(
        &mut self,
        context: &C,
    ) -> Result<()> {
        let interval = context.config()?.build_workspace_reinitialization_interval;
        if self.workspace_initialize_time.elapsed() >= interval {
            info!("start reinitialize workspace again");
            self.workspace = build_workspace(context)?;
            self.workspace_initialize_time = Instant::now();
        }

        Ok(())
    }

    #[instrument(skip(self))]
    fn prepare_sandbox(&self, limits: &Limits) -> SandboxBuilder {
        SandboxBuilder::new()
            .cpu_limit(self.config.build_cpu_limit.map(|limit| limit as f32))
            .memory_limit(Some(limits.memory()))
            .enable_networking(limits.networking())
    }

    pub fn purge_caches(&self) -> Result<()> {
        self.workspace.purge_all_caches()?;
        Ok(())
    }

    pub fn update_toolchain(&mut self) -> Result<bool> {
        self.toolchain = self.runtime.block_on(async {
            let mut conn = self.db.get_async().await?;
            get_configured_toolchain(&mut conn).await
        })?;

        // For CI builds, a lot of the normal update_toolchain things don't apply.
        // CI builds are only for one platform (https://forge.rust-lang.org/infra/docs/rustc-ci.html#try-builds)
        // so we only try installing for the current platform. If that's not a match,
        // for instance if we're running on macOS or Windows, this will error.
        // Also, detecting the rustc version relies on calling rustc through rustup with the
        // +channel argument, but the +channel argument doesn't work for CI builds. So
        // we fake the rustc version and install from scratch every time since we can't detect
        // the already-installed rustc version.
        if self.toolchain.as_ci().is_some() {
            self.toolchain.install(&self.workspace)?;
            self.add_essential_files()?;
            return Ok(true);
        }

        // Ignore errors if detection fails.
        let old_version = self.detect_rustc_version().ok();

        let mut targets_to_install = DEFAULT_TARGETS
            .iter()
            .map(|&t| t.to_string()) // &str has a specialized ToString impl, while &&str goes through Display
            .collect::<HashSet<_>>();

        let installed_targets = match self.toolchain.installed_targets(&self.workspace) {
            Ok(targets) => targets,
            Err(err) => {
                if let Some(&ToolchainError::NotInstalled) = err.downcast_ref::<ToolchainError>() {
                    Vec::new()
                } else {
                    return Err(err);
                }
            }
        };

        // The extra targets are intentionally removed *before* trying to update.
        //
        // If a target is installed locally and it goes missing the next update, rustup will block
        // the update to avoid leaving the system in a broken state. This is not a behavior we want
        // though when we also remove the target from the list managed by docs.rs: we want that
        // target gone, and we don't care if it's missing in the next update.
        //
        // Removing it beforehand works fine, and prevents rustup from blocking the update later in
        // the method.
        //
        // Note that this means that non tier-one targets will be uninstalled on every update,
        // and will not be reinstalled until explicitly requested by a crate.
        for target in installed_targets {
            if !targets_to_install.remove(&target) {
                self.toolchain.remove_target(&self.workspace, &target)?;
            }
        }

        self.toolchain.install(&self.workspace)?;

        for target in &targets_to_install {
            self.toolchain.add_target(&self.workspace, target)?;
        }
        // NOTE: rustup will automatically refuse to update the toolchain
        // if `rustfmt` is not available in the newer version
        // NOTE: this ignores the error so that you can still run a build without rustfmt.
        // This should only happen if you run a build for the first time when rustfmt isn't available.
        for component in COMPONENTS {
            if let Err(err) = self.toolchain.add_component(&self.workspace, component) {
                warn!("failed to install {component}: {err}");
                info!("continuing anyway, since this must be the first build");
            }
        }

        let has_changed = old_version != Some(self.rustc_version()?);
        Ok(has_changed)
    }

    fn rustc_version(&self) -> Result<String> {
        let version = self
            .toolchain
            .as_ci()
            .map(|ci| {
                // Detecting the rustc version relies on calling rustc through rustup with the
                // +channel argument, but the +channel argument doesn't work for CI builds. So
                // we fake the rustc version.
                Ok(format!("rustc 1.9999.0-nightly ({} 2999-12-29)", ci.sha()))
            })
            .unwrap_or_else(|| self.detect_rustc_version())?;
        Ok(version)
    }

    /// Return a string containing the output of `rustc --version`. Only valid
    /// for dist toolchains. Will error if run with a CI toolchain.
    fn detect_rustc_version(&self) -> Result<String> {
        info!("detecting rustc's version...");
        let res = Command::new(&self.workspace, self.toolchain.rustc())
            .args(&["--version"])
            .log_output(false)
            .run_capture()?;
        let mut iter = res.stdout_lines().iter();
        if let (Some(line), None) = (iter.next(), iter.next()) {
            info!("found rustc {}", line);
            Ok(line.clone())
        } else {
            Err(anyhow!("invalid output returned by `rustc --version`",))
        }
    }

    #[instrument(skip(self))]
    fn get_limits(&self, krate: &str) -> Result<Limits> {
        self.runtime.block_on({
            let db = self.db.clone();
            let config = self.config.clone();
            async move {
                let mut conn = db.get_async().await?;
                Limits::for_crate(&config, &mut conn, krate).await
            }
        })
    }

    pub fn add_essential_files(&mut self) -> Result<()> {
        let rustc_version = self.rustc_version()?;
        let parsed_rustc_version = parse_rustc_version(&rustc_version)?;

        info!("building a dummy crate to get essential files");

        let limits = self.get_limits(DUMMY_CRATE_NAME)?;

        // FIXME: for now, purge all build dirs before each build.
        // Currently we have some error situations where the build directory wouldn't be deleted
        // after the build failed:
        // https://github.com/rust-lang/docs.rs/issues/820
        // This should be solved in a better way, likely refactoring the whole builder structure,
        // but for now we chose this simple way to prevent that the build directory remains can
        // fill up disk space.
        // This also prevents having multiple builders using the same rustwide workspace,
        // which we don't do. Currently our separate builders use a separate rustwide workspace.
        self.workspace.purge_all_build_dirs()?;

        let mut build_dir = self
            .workspace
            .build_dir(&format!("essential-files-{parsed_rustc_version}"));

        // This is an empty library crate that is supposed to always build.
        let krate = Crate::crates_io(DUMMY_CRATE_NAME, DUMMY_CRATE_VERSION);
        krate.fetch(&self.workspace)?;

        build_dir
            .build(&self.toolchain, &krate, self.prepare_sandbox(&limits))
            .run(|build| {
                let metadata = Metadata::from_crate_root(build.host_source_dir())?;

                let res = self.execute_build(
                    BuildId(0),
                    DUMMY_CRATE_NAME,
                    DUMMY_CRATE_VERSION,
                    HOST_TARGET,
                    true,
                    build,
                    &limits,
                    &metadata,
                    true,
                    false,
                )?;
                if !res.result.successful {
                    bail!("failed to build dummy crate for {}", rustc_version);
                }

                info!("copying essential files for {}", rustc_version);
                assert!(!metadata.proc_macro);
                let source = build.host_target_dir().join(HOST_TARGET).join("doc");
                let dest = tempfile::Builder::new()
                    .prefix("essential-files")
                    .tempdir()?;
                copy_dir_all(source, &dest)?;

                // One https://github.com/rust-lang/rust/pull/101702 lands, static files will be
                // put in their own directory, "static.files". To make sure those files are
                // available at --static-root-path, we add files from that subdirectory, if present.
                let static_files = dest.as_ref().join("static.files");
                if static_files.try_exists()? {
                    self.runtime.block_on(add_path_into_database(
                        &self.async_storage,
                        RUSTDOC_STATIC_STORAGE_PREFIX,
                        &static_files,
                    ))?;
                } else {
                    self.runtime.block_on(add_path_into_database(
                        &self.async_storage,
                        RUSTDOC_STATIC_STORAGE_PREFIX,
                        &dest,
                    ))?;
                }

                self.runtime.block_on(async {
                    let mut conn = self.db.get_async().await?;
                    set_config(&mut conn, ConfigName::RustcVersion, rustc_version).await
                })?;
                Ok(())
            })?;

        krate.purge_from_cache(&self.workspace)?;
        Ok(())
    }

    pub fn build_local_package(&mut self, path: &Path) -> Result<BuildPackageSummary> {
        let metadata = CargoMetadata::load_from_rustwide(&self.workspace, &self.toolchain, path)
            .map_err(|err| {
                err.context(format!("failed to load local package {}", path.display()))
            })?;
        let package = metadata.root();
        self.build_package(
            &package.name,
            &package.version,
            PackageKind::Local(path),
            false,
        )
    }

    #[instrument(name = "docbuilder.build_package", parent = None, skip(self, name), fields(krate=name))]
    pub fn build_package(
        &mut self,
        name: &str,
        version: &str,
        kind: PackageKind<'_>,
        collect_metrics: bool,
    ) -> Result<BuildPackageSummary> {
        let (crate_id, release_id, build_id) = self.runtime.block_on(async {
            let mut conn = self.db.get_async().await?;
            let crate_id = initialize_crate(&mut conn, name).await?;
            let release_id = initialize_release(&mut conn, crate_id, version).await?;
            let build_id = initialize_build(&mut conn, release_id).await?;
            Ok::<_, Error>((crate_id, release_id, build_id))
        })?;

        match self.build_package_inner(
            name,
            version,
            kind,
            crate_id,
            release_id,
            build_id,
            collect_metrics,
        ) {
            Ok(successful) => Ok(BuildPackageSummary {
                successful,
                should_reattempt: false,
            }),
            Err(err) => self.runtime.block_on(async {
                // NOTE: this might hide some errors from us, while only surfacing them in the build
                // result.
                // At some point we might introduce a special error type which additionally reports
                // to sentry.
                let mut conn = self.db.get_async().await?;

                update_build_with_error(&mut conn, build_id, Some(&format!("{err:?}"))).await?;

                Ok(BuildPackageSummary {
                    successful: false,
                    should_reattempt: true,
                })
            }),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_package_inner(
        &mut self,
        name: &str,
        version: &str,
        kind: PackageKind<'_>,
        crate_id: CrateId,
        release_id: ReleaseId,
        build_id: BuildId,
        collect_metrics: bool,
    ) -> Result<bool> {
        info!("building package {} {}", name, version);

        let is_blacklisted = self.runtime.block_on(async {
            let mut conn = self.db.get_async().await?;

            is_blacklisted(&mut conn, name).await
        })?;

        if is_blacklisted {
            info!("skipping build of {}, crate has been blacklisted", name);
            return Ok(false);
        }

        let limits = self.get_limits(name)?;
        #[cfg(target_os = "linux")]
        if !self.config.disable_memory_limit {
            use anyhow::Context;
            let mem_info = procfs::Meminfo::new().context("failed to read /proc/meminfo")?;
            let available = mem_info
                .mem_available
                .expect("kernel version too old for determining memory limit");
            if limits.memory() as u64 > available {
                bail!(
                    "not enough memory to build {} {}: needed {} MiB, have {} MiB\nhelp: set DOCSRS_DISABLE_MEMORY_LIMIT=true to force a build",
                    name,
                    version,
                    limits.memory() / 1024 / 1024,
                    available / 1024 / 1024
                );
            } else {
                debug!(
                    "had enough memory: {} MiB <= {} MiB",
                    limits.memory() / 1024 / 1024,
                    available / 1024 / 1024
                );
            }
        }

        // FIXME: for now, purge all build dirs before each build.
        // Currently we have some error situations where the build directory wouldn't be deleted
        // after the build failed:
        // https://github.com/rust-lang/docs.rs/issues/820
        // This should be solved in a better way, likely refactoring the whole builder structure,
        // but for now we chose this simple way to prevent that the build directory remains can
        // fill up disk space.
        // This also prevents having multiple builders using the same rustwide workspace,
        // which we don't do. Currently our separate builders use a separate rustwide workspace.
        info_span!("purge_all_build_dirs").in_scope(|| self.workspace.purge_all_build_dirs())?;

        let mut build_dir = self.workspace.build_dir(&format!("{name}-{version}"));

        let is_local = matches!(kind, PackageKind::Local(_));
        let krate = {
            let _span = info_span!("krate.fetch").entered();

            let krate = match kind {
                PackageKind::Local(path) => Crate::local(path),
                PackageKind::CratesIo => Crate::crates_io(name, version),
                PackageKind::Registry(registry) => {
                    Crate::registry(AlternativeRegistry::new(registry), name, version)
                }
            };
            krate.fetch(&self.workspace)?;
            krate
        };

        fs::create_dir_all(&self.config.temp_dir)?;
        let local_storage = tempfile::tempdir_in(&self.config.temp_dir)?;

        let successful = build_dir
            .build(&self.toolchain, &krate, self.prepare_sandbox(&limits))
            .run(|build| {
                let mut algs = HashSet::new();

                debug!("adding sources into database");
                let files_list = {
                    let (files_list, new_alg) =
                        self.runtime.block_on(add_path_into_remote_archive(
                            &self.async_storage,
                            &source_archive_path(name, version),
                            build.host_source_dir(),
                            false,
                        ))?;
                    algs.insert(new_alg);
                    files_list
                };
                let source_size: u64 = files_list.iter().map(|info| info.size).sum();
                let metadata = Metadata::from_crate_root(build.host_source_dir())?;
                let BuildTargets {
                    default_target,
                    other_targets,
                } = metadata.targets(self.config.include_default_targets);
                let mut targets = vec![default_target];
                targets.extend(&other_targets);

                {
                    let _span = info_span!("fetch_build_std_dependencies").entered();
                    // Fetch this before we enter the sandbox, so networking isn't blocked.
                    build.fetch_build_std_dependencies(&targets)?;
                }


                let mut has_docs = false;
                let mut successful_targets = Vec::new();

                // Perform an initial build
                let mut res =
                    self.execute_build(build_id, name, version, default_target, true, build, &limits, &metadata, false, collect_metrics)?;

                // If the build fails with the lockfile given, try using only the dependencies listed in Cargo.toml.
                let cargo_lock = build.host_source_dir().join("Cargo.lock");
                if !res.result.successful && cargo_lock.exists() {
                    info!("removing lockfile and reattempting build");
                    std::fs::remove_file(cargo_lock)?;
                    {
                        let _span = info_span!("cargo_generate_lockfile").entered();
                        Command::new(&self.workspace, self.toolchain.cargo())
                            .cd(build.host_source_dir())
                            .args(&["generate-lockfile"])
                            .run_capture()?;
                    }
                    {
                        let _span = info_span!("cargo fetch --locked").entered();
                        Command::new(&self.workspace, self.toolchain.cargo())
                            .cd(build.host_source_dir())
                            .args(&["fetch", "--locked"])
                            .run_capture()?;
                    }
                    res =
                        self.execute_build(build_id, name, version, default_target, true, build, &limits, &metadata, false, collect_metrics)?;
                }

                if res.result.successful {
                    if let Some(name) = res.cargo_metadata.root().library_name() {
                        let host_target = build.host_target_dir();
                        has_docs = host_target
                            .join(default_target)
                            .join("doc")
                            .join(name)
                            .is_dir();
                    }
                }

                let mut target_build_logs = HashMap::new();
                let documentation_size = if has_docs {
                    debug!("adding documentation for the default target to the database");
                    self.copy_docs(
                        &build.host_target_dir(),
                        local_storage.path(),
                        default_target,
                        true,
                    )?;

                    successful_targets.push(res.target.clone());

                    // Then build the documentation for all the targets
                    // Limit the number of targets so that no one can try to build all 200000 possible targets
                    for target in other_targets.into_iter().take(limits.targets()) {
                        debug!("building package {} {} for {}", name, version, target);
                        let target_res = self.build_target(
                            build_id,
                            name,
                            version,
                            target,
                            build,
                            &limits,
                            local_storage.path(),
                            &mut successful_targets,
                            &metadata,
                            collect_metrics,
                        )?;
                        target_build_logs.insert(target, target_res.build_log);
                    }
                    let (file_list, new_alg) =
                        self.runtime.block_on(add_path_into_remote_archive(
                            &self.async_storage,
                            &rustdoc_archive_path(name, version),
                            local_storage.path(),
                            true,
                        ))?;
                    let documentation_size = file_list.iter().map(|info| info.size).sum::<u64>();
                    self.metrics
                        .documentation_size
                        .observe(documentation_size as f64 / 1024.0 / 1024.0);
                    algs.insert(new_alg);
                    Some(documentation_size)
                } else {
                    None
                };

                let mut async_conn = self.runtime.block_on(self.db.get_async())?;

                self.runtime.block_on(finish_build(
                    &mut async_conn,
                    build_id,
                    &res.result.rustc_version,
                    &res.result.docsrs_version,
                    if res.result.successful {
                        BuildStatus::Success
                    } else {
                        BuildStatus::Failure
                    },
                    documentation_size,
                    None,
                ))?;

                {
                    let _span = info_span!("store_build_logs").entered();
                    let build_log_path = format!("build-logs/{build_id}/{default_target}.txt");
                    self.storage.store_one(build_log_path, res.build_log)?;
                    for (target, log) in target_build_logs {
                        let build_log_path = format!("build-logs/{build_id}/{target}.txt");
                        self.storage.store_one(build_log_path, log)?;
                    }
                }

                if res.result.successful {
                    self.metrics.successful_builds.inc();
                } else if res.cargo_metadata.root().is_library() {
                    self.metrics.failed_builds.inc();
                } else {
                    self.metrics.non_library_builds.inc();
                }

                let release_data = if !is_local {
                    match self
                        .runtime
                        .block_on(self.registry_api.get_release_data(name, version))
                        .with_context(|| {
                            format!("could not fetch releases-data for {name}-{version}")
                        }) {
                        Ok(data) => Some(data),
                        Err(err) => {
                            report_error(&err);
                            None
                        }
                    }
                } else {
                    None
                }
                .unwrap_or_default();

                let cargo_metadata = res.cargo_metadata.root();
                let repository = self.get_repo(cargo_metadata)?;

                // when we have an unsuccessful build, but the release was already successfullly
                // built in the past, don't touch the release record so the docs stay intact.
                // This mainly happens with manually triggered or automated rebuilds.
                // The `release_build_status` table is already updated with the information from
                // the current build via `finish_build`.
                let current_release_build_status = self.runtime.block_on(sqlx::query_scalar!(
                    r#"
                    SELECT build_status AS "build_status: BuildStatus"
                    FROM release_build_status
                    WHERE rid = $1
                    "#,
                    release_id.0,
                ).fetch_optional(&mut *async_conn))?;

                if !res.result.successful && current_release_build_status == Some(BuildStatus::Success) {
                    info!("build was unsuccessful, but the release was already successfully built in the past. Skipping release record update.");
                    return Ok(false);
                }

                let has_examples = build.host_source_dir().join("examples").is_dir();
                self.runtime.block_on(finish_release(
                    &mut async_conn,
                    crate_id,
                    release_id,
                    cargo_metadata,
                    &build.host_source_dir(),
                    &res.target,
                    file_list_to_json(files_list),
                    successful_targets,
                    &release_data,
                    has_docs,
                    has_examples,
                    algs,
                    repository,
                    true,
                    source_size,
                ))?;

                if let Some(doc_coverage) = res.doc_coverage {
                    self.runtime.block_on(add_doc_coverage(
                        &mut async_conn,
                        release_id,
                        doc_coverage,
                    ))?;
                }

                // Some crates.io crate data is mutable, so we proactively update it during a release
                if !is_local {
                    match self
                        .runtime
                        .block_on(self.registry_api.get_crate_data(name))
                    {
                        Ok(crate_data) => self.runtime.block_on(update_crate_data_in_database(
                            &mut async_conn,
                            name,
                            &crate_data,
                        ))?,
                        Err(err) => warn!("{:#?}", err),
                    }
                }

                if res.result.successful {
                    // delete eventually existing files from pre-archive storage.
                    // we're doing this in the end so eventual problems in the build
                    // won't lead to non-existing docs.
                    for prefix in &["rustdoc", "sources"] {
                        let prefix = format!("{prefix}/{name}/{version}/");
                        debug!("cleaning old storage folder {}", prefix);
                        self.storage.delete_prefix(&prefix)?;
                    }
                }

                self.runtime.block_on(async move {
                    // we need to drop the async connection inside an async runtime context
                    // so sqlx can use a runtime to handle the pool.
                    drop(async_conn);
                });

                Ok(res.result.successful)
            })?;

        {
            let _span = info_span!("purge_from_cache").entered();
            krate.purge_from_cache(&self.workspace)?;
            local_storage.close()?;
        }
        Ok(successful)
    }

    #[instrument(skip(self, build))]
    #[allow(clippy::too_many_arguments)]
    fn build_target(
        &self,
        build_id: BuildId,
        name: &str,
        version: &str,
        target: &str,
        build: &Build,
        limits: &Limits,
        local_storage: &Path,
        successful_targets: &mut Vec<String>,
        metadata: &Metadata,
        collect_metrics: bool,
    ) -> Result<FullBuildResult> {
        let target_res = self.execute_build(
            build_id,
            name,
            version,
            target,
            false,
            build,
            limits,
            metadata,
            false,
            collect_metrics,
        )?;
        if target_res.result.successful {
            // Cargo is not giving any error and not generating documentation of some crates
            // when we use a target compile options. Check documentation exists before
            // adding target to successfully_targets.
            if build.host_target_dir().join(target).join("doc").is_dir() {
                debug!("adding documentation for target {} to the database", target,);
                self.copy_docs(&build.host_target_dir(), local_storage, target, false)?;
                successful_targets.push(target.to_string());
            }
        }
        Ok(target_res)
    }

    /// Run the build with rustdoc JSON output for a specific target and directly upload the
    /// build log & the JSON files.
    ///
    /// The method only returns an `Err` for internal errors that should be retryable.
    /// For all build errors we would just upload the log file and still return `Ok(())`.
    #[instrument(skip(self, build))]
    #[allow(clippy::too_many_arguments)]
    fn execute_json_build(
        &self,
        build_id: BuildId,
        name: &str,
        version: &str,
        target: &str,
        is_default_target: bool,
        build: &Build,
        metadata: &Metadata,
        limits: &Limits,
    ) -> Result<()> {
        let rustdoc_flags = vec!["--output-format".to_string(), "json".to_string()];

        let mut storage = LogStorage::new(log::LevelFilter::Info);
        storage.set_max_size(limits.max_log_size());

        let successful = logging::capture(&storage, || {
            let _span = info_span!("cargo_build_json", target = %target).entered();
            self.prepare_command(build, target, metadata, limits, rustdoc_flags, false)
                .and_then(|command| command.run().map_err(Error::from))
                .is_ok()
        });

        {
            let _span = info_span!("store_json_build_logs").entered();
            let build_log_path = format!("build-logs/{build_id}/{target}_json.txt");
            self.storage
                .store_one(build_log_path, storage.to_string())
                .context("storing build log on S3")?;
        }

        if !successful {
            // this is a normal build error and will be visible in the uploaded build logs.
            // We don't need the Err variant here.
            return Ok(());
        }

        let json_dir = if metadata.proc_macro {
            assert!(
                is_default_target && target == HOST_TARGET,
                "can't handle cross-compiling macros"
            );
            build.host_target_dir().join("doc")
        } else {
            build.host_target_dir().join(target).join("doc")
        };

        let json_filename = fs::read_dir(&json_dir)?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.is_file() && path.extension()? == "json" {
                    Some(path)
                } else {
                    None
                }
            })
            .next()
            .ok_or_else(|| {
                anyhow!(
                    "no JSON file found in target/doc after successful rustdoc json build.\n\
                     search directory: {}\n\
                     files: {:?}",
                    json_dir.to_string_lossy(),
                    get_file_list(&json_dir)
                        .filter_map(Result::ok)
                        .map(|p| p.to_string_lossy().to_string())
                        .collect_vec(),
                )
            })?;

        let format_version = {
            let _span = info_span!("read_format_version").entered();
            read_format_version_from_rustdoc_json(&File::open(&json_filename)?)
                .context("couldn't parse rustdoc json to find format version")?
        };

        for alg in RUSTDOC_JSON_COMPRESSION_ALGORITHMS {
            let compressed_json: Vec<u8> = {
                let _span =
                    info_span!("compress_json", file_size = json_filename.metadata()?.len(), algorithm=%alg)
                        .entered();

                compress(BufReader::new(File::open(&json_filename)?), *alg)?
            };

            for format_version in [format_version, RustdocJsonFormatVersion::Latest] {
                let path = rustdoc_json_path(name, version, target, format_version, Some(*alg));
                let _span =
                    info_span!("store_json", %format_version, algorithm=%alg, target_path=%path)
                        .entered();

                self.storage
                    .store_one_uncompressed(&path, compressed_json.clone())?;
                self.storage.set_public_access(&path, true)?;
            }
        }

        Ok(())
    }

    #[instrument(skip(self, build))]
    fn get_coverage(
        &self,
        target: &str,
        build: &Build,
        metadata: &Metadata,
        limits: &Limits,
    ) -> Result<Option<DocCoverage>> {
        let rustdoc_flags = vec![
            "--output-format".to_string(),
            "json".to_string(),
            "--show-coverage".to_string(),
        ];

        #[derive(serde::Deserialize)]
        struct FileCoverage {
            total: i32,
            with_docs: i32,
            total_examples: i32,
            with_examples: i32,
        }

        let mut coverage = DocCoverage {
            total_items: 0,
            documented_items: 0,
            total_items_needing_examples: 0,
            items_with_examples: 0,
        };

        self.prepare_command(build, target, metadata, limits, rustdoc_flags, false)?
            .process_lines(&mut |line, _| {
                if line.starts_with('{') && line.ends_with('}') {
                    let parsed = match serde_json::from_str::<HashMap<String, FileCoverage>>(line) {
                        Ok(parsed) => parsed,
                        Err(_) => return,
                    };
                    for file in parsed.values() {
                        coverage.total_items += file.total;
                        coverage.documented_items += file.with_docs;
                        coverage.total_items_needing_examples += file.total_examples;
                        coverage.items_with_examples += file.with_examples;
                    }
                }
            })
            .log_output(true)
            .run()?;

        Ok(
            if coverage.total_items == 0 && coverage.documented_items == 0 {
                None
            } else {
                Some(coverage)
            },
        )
    }

    #[instrument(skip(self, build))]
    #[allow(clippy::too_many_arguments)]
    fn execute_build(
        &self,
        build_id: BuildId,
        name: &str,
        version: &str,
        target: &str,
        is_default_target: bool,
        build: &Build,
        limits: &Limits,
        metadata: &Metadata,
        create_essential_files: bool,
        collect_metrics: bool,
    ) -> Result<FullBuildResult> {
        let cargo_metadata = CargoMetadata::load_from_rustwide(
            &self.workspace,
            &self.toolchain,
            &build.host_source_dir(),
        )?;

        let mut rustdoc_flags = vec![
            if create_essential_files {
                "--emit=unversioned-shared-resources,toolchain-shared-resources"
            } else {
                "--emit=invocation-specific"
            }
            .to_string(),
        ];
        rustdoc_flags.extend(vec![
            "--resource-suffix".to_string(),
            format!("-{}", parse_rustc_version(self.rustc_version()?)?),
        ]);

        let mut storage = LogStorage::new(log::LevelFilter::Info);
        storage.set_max_size(limits.max_log_size());

        // we have to run coverage before the doc-build because currently it
        // deletes the doc-target folder.
        // https://github.com/rust-lang/cargo/issues/9447
        let doc_coverage = match self.get_coverage(target, build, metadata, limits) {
            Ok(cov) => cov,
            Err(err) => {
                info!("error when trying to get coverage: {}", err);
                info!("continuing anyways.");
                None
            }
        };

        if let Err(err) = self.execute_json_build(
            build_id,
            name,
            version,
            target,
            is_default_target,
            build,
            metadata,
            limits,
        ) {
            // FIXME: this is temporary. Theoretically all `Err` things coming out
            // of the method should be retryable, so we could juse use `?` here.
            // But since this is new, I want to be carful and first see what kind of
            // errors we are seeing here.
            error!(
                ?err,
                "internal error when trying to generate rustdoc JSON output"
            );
        }

        let successful = {
            let _span = info_span!("cargo_build", target = %target, is_default_target).entered();
            logging::capture(&storage, || {
                self.prepare_command(
                    build,
                    target,
                    metadata,
                    limits,
                    rustdoc_flags,
                    collect_metrics,
                )
                .and_then(|command| command.run().map_err(Error::from))
                .is_ok()
            })
        };

        if collect_metrics {
            if let Some(compiler_metric_target_dir) = &self.config.compiler_metrics_collection_path
            {
                let metric_output = build.host_target_dir().join("metrics/");
                info!(
                    "found {} files in metric dir, copy over to {} (exists: {})",
                    fs::read_dir(&metric_output)?.count(),
                    &compiler_metric_target_dir.to_string_lossy(),
                    &compiler_metric_target_dir.exists(),
                );
                copy_dir_all(&metric_output, compiler_metric_target_dir)?;
                fs::remove_dir_all(&metric_output)?;
            }
        }

        // For proc-macros, cargo will put the output in `target/doc`.
        // Move it to the target-specific directory for consistency with other builds.
        // NOTE: don't rename this if the build failed, because `target/doc` won't exist.
        if successful && metadata.proc_macro {
            assert!(
                is_default_target && target == HOST_TARGET,
                "can't handle cross-compiling macros"
            );
            // mv target/doc target/$target/doc
            let target_dir = build.host_target_dir();
            let old_dir = target_dir.join("doc");
            let new_dir = target_dir.join(target).join("doc");
            debug!("rename {} to {}", old_dir.display(), new_dir.display());
            std::fs::create_dir(target_dir.join(target))?;
            std::fs::rename(old_dir, new_dir)?;
        }

        Ok(FullBuildResult {
            result: BuildResult {
                rustc_version: self.rustc_version()?,
                docsrs_version: format!("docsrs {}", crate::BUILD_VERSION),
                successful,
            },
            doc_coverage,
            cargo_metadata,
            build_log: storage.to_string(),
            target: target.to_string(),
        })
    }

    fn prepare_command<'ws, 'pl>(
        &self,
        build: &'ws Build,
        target: &str,
        metadata: &Metadata,
        limits: &Limits,
        mut rustdoc_flags_extras: Vec<String>,
        collect_metrics: bool,
    ) -> Result<Command<'ws, 'pl>> {
        // Add docs.rs specific arguments
        let mut cargo_args = vec![
            "--offline".into(),
            // We know that `metadata` unconditionally passes `-Z rustdoc-map`.
            // Don't copy paste this, since that fact is not stable and may change in the future.
            "-Zunstable-options".into(),
            // Add `target` so that if a dependency has target-specific docs, this links to them properly.
            //
            // Note that this includes the target even if this is the default, since the dependency
            // may have a different default (and the web backend will take care of redirecting if
            // necessary).
            //
            // FIXME: host-only crates like proc-macros should probably not have this passed? but #1417 should make it OK
            format!(
                r#"--config=doc.extern-map.registries.crates-io="https://docs.rs/{{pkg_name}}/{{version}}/{target}""#
            ),
            // Enables the unstable rustdoc-scrape-examples feature. We are "soft launching" this feature on
            // docs.rs, but once it's stable we can remove this flag.
            "-Zrustdoc-scrape-examples".into(),
        ];
        if let Some(cpu_limit) = self.config.build_cpu_limit {
            cargo_args.push(format!("-j{cpu_limit}"));
        }
        // Cargo has a series of frightening bugs around cross-compiling proc-macros:
        // - Passing `--target` causes RUSTDOCFLAGS to fail to be passed 🤦
        // - Passing `--target` will *create* `target/{target-name}/doc` but will put the docs in `target/doc` anyway
        // As a result, it's not possible for us to support cross-compiling proc-macros.
        // However, all these caveats unfortunately still apply when `{target-name}` is the host.
        // So, only pass `--target` for crates that aren't proc-macros.
        //
        // Originally, this had a simpler check `target != HOST_TARGET`, but *that* was buggy when `HOST_TARGET` wasn't the same as the default target.
        // Rather than trying to keep track of it all, only special case proc-macros, which are what we actually care about.
        if !metadata.proc_macro {
            cargo_args.push("--target".into());
            cargo_args.push(target.into());
        };

        #[rustfmt::skip]
        const UNCONDITIONAL_ARGS: &[&str] = &[
            "--static-root-path", "/-/rustdoc.static/",
            "--cap-lints", "warn",
            "--extern-html-root-takes-precedence",
        ];

        rustdoc_flags_extras.extend(UNCONDITIONAL_ARGS.iter().map(|&s| s.to_owned()));
        let mut cargo_args = metadata.cargo_args(&cargo_args, &rustdoc_flags_extras);

        // If the explicit target is not a tier one target, we need to install it.
        let has_build_std = cargo_args.windows(2).any(|args| {
            args[0].starts_with("-Zbuild-std")
                || (args[0] == "-Z" && args[1].starts_with("build-std"))
        }) || cargo_args.last().unwrap().starts_with("-Zbuild-std");
        if !docsrs_metadata::DEFAULT_TARGETS.contains(&target) && !has_build_std {
            // This is a no-op if the target is already installed.
            self.toolchain.add_target(&self.workspace, target)?;
        }

        let mut command = build
            .cargo()
            .timeout(Some(limits.timeout()))
            .no_output_timeout(None);

        for (key, val) in metadata.environment_variables() {
            command = command.env(key, val);
        }

        if collect_metrics && self.config.compiler_metrics_collection_path.is_some() {
            // set the `./target/metrics/` directory inside the build container
            // as a target directory for the metric files.
            let flag = "-Zmetrics-dir=/opt/rustwide/target/metrics";

            // this is how we can reach it from outside the container.
            fs::create_dir_all(build.host_target_dir().join("metrics/"))?;

            let rustdocflags = toml::Value::try_from(vec![flag])
                .expect("serializing a string should never fail")
                .to_string();
            cargo_args.push("--config".into());
            cargo_args.push(format!("build.rustdocflags={rustdocflags}"));
        }

        Ok(command.args(&cargo_args))
    }

    #[instrument(skip(self))]
    fn copy_docs(
        &self,
        target_dir: &Path,
        local_storage: &Path,
        target: &str,
        is_default_target: bool,
    ) -> Result<()> {
        let source = target_dir.join(target).join("doc");

        let mut dest = local_storage.to_path_buf();
        // only add target name to destination directory when we are copying a non-default target.
        // this is allowing us to host documents in the root of the crate documentation directory.
        // for example winapi will be available in docs.rs/winapi/$version/winapi/ for it's
        // default target: x86_64-pc-windows-msvc. But since it will be built under
        // target/x86_64-pc-windows-msvc we still need target in this function.
        if !is_default_target {
            dest = dest.join(target);
        }

        info!("copy {} to {}", source.display(), dest.display());
        copy_dir_all(source, dest).map_err(Into::into)
    }

    fn get_repo(&self, metadata: &MetadataPackage) -> Result<Option<i32>> {
        self.runtime
            .block_on(self.repository_stats_updater.load_repository(metadata))
    }
}

struct FullBuildResult {
    result: BuildResult,
    target: String,
    cargo_metadata: CargoMetadata,
    doc_coverage: Option<DocCoverage>,
    build_log: String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DocCoverage {
    /// The total items that could be documented in the current crate, used to calculate
    /// documentation coverage.
    pub(crate) total_items: i32,
    /// The items of the crate that are documented, used to calculate documentation coverage.
    pub(crate) documented_items: i32,
    /// The total items that could have code examples in the current crate, used to calculate
    /// documentation coverage.
    pub(crate) total_items_needing_examples: i32,
    /// The items of the crate that have a code example, used to calculate documentation coverage.
    pub(crate) items_with_examples: i32,
}

#[derive(Debug)]
pub(crate) struct BuildResult {
    pub(crate) rustc_version: String,
    pub(crate) docsrs_version: String,
    pub(crate) successful: bool,
}

#[derive(Debug)]
pub struct BuildPackageSummary {
    pub successful: bool,
    pub should_reattempt: bool,
}

#[cfg(test)]
impl Default for BuildPackageSummary {
    fn default() -> Self {
        Self {
            successful: true,
            should_reattempt: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::types::Feature;
    use crate::registry_api::ReleaseData;
    use crate::storage::{CompressionAlgorithm, compression};
    use crate::test::{AxumRouterTestExt, TestEnvironment, wrapper};
    use std::{io, iter};
    use test_case::test_case;

    fn get_features(
        env: &TestEnvironment,
        name: &str,
        version: &str,
    ) -> Result<Option<Vec<Feature>>, sqlx::Error> {
        env.runtime().block_on(async {
            let mut conn = env.async_db().await.async_conn().await;
            sqlx::query_scalar!(
                r#"SELECT
                        releases.features "features?: Vec<Feature>"
                     FROM releases
                     INNER JOIN crates ON crates.id = releases.crate_id
                     WHERE crates.name = $1 AND releases.version = $2"#,
                name,
                version,
            )
            .fetch_one(&mut *conn)
            .await
        })
    }

    fn remove_cache_files(env: &TestEnvironment, crate_: &str, version: &str) -> Result<()> {
        let paths = [
            format!("cache/index.crates.io-6f17d22bba15001f/{crate_}-{version}.crate"),
            format!("src/index.crates.io-6f17d22bba15001f/{crate_}-{version}"),
            format!(
                "index/index.crates.io-6f17d22bba15001f/.cache/{}/{}/{crate_}",
                &crate_[0..2],
                &crate_[2..4]
            ),
        ];

        for path in paths {
            let full_path = env
                .config()
                .rustwide_workspace
                .join("cargo-home/registry")
                .join(path);
            if full_path.exists() {
                info!("deleting {}", full_path.display());
                if full_path.is_file() {
                    std::fs::remove_file(full_path)?;
                } else {
                    std::fs::remove_dir_all(full_path)?;
                }
            }
        }

        Ok(())
    }

    #[test]
    #[ignore]
    fn test_build_crate() {
        wrapper(|env| {
            let crate_ = DUMMY_CRATE_NAME;
            let crate_path = crate_.replace('-', "_");
            let version = DUMMY_CRATE_VERSION;
            let default_target = "x86_64-unknown-linux-gnu";

            let storage = env.storage();
            let old_rustdoc_file = format!("rustdoc/{crate_}/{version}/some_doc_file");
            let old_source_file = format!("sources/{crate_}/{version}/some_source_file");
            storage.store_one(&old_rustdoc_file, Vec::new())?;
            storage.store_one(&old_source_file, Vec::new())?;

            let mut builder = RustwideBuilder::init(env).unwrap();
            builder.update_toolchain()?;
            assert!(
                builder
                    .build_package(crate_, version, PackageKind::CratesIo, false)?
                    .successful
            );

            // check release record in the db (default and other targets)
            let row = env.runtime().block_on(async {
                let mut conn = env.async_db().await.async_conn().await;
                sqlx::query!(
                    r#"SELECT
                        r.rustdoc_status,
                        r.default_target,
                        r.doc_targets,
                        r.archive_storage,
                        r.source_size as "source_size!",
                        cov.total_items,
                        b.id as build_id,
                        b.build_status::TEXT as build_status,
                        b.docsrs_version,
                        b.rustc_version,
                        b.documentation_size
                    FROM
                        crates as c
                        INNER JOIN releases AS r ON c.id = r.crate_id
                        INNER JOIN builds as b ON r.id = b.rid
                        LEFT OUTER JOIN doc_coverage AS cov ON r.id = cov.release_id
                    WHERE
                        c.name = $1 AND
                        r.version = $2"#,
                    crate_,
                    version,
                )
                .fetch_one(&mut *conn)
                .await
            })?;

            assert_eq!(row.rustdoc_status, Some(true));
            assert_eq!(row.default_target, Some(default_target.into()));
            assert!(row.total_items.is_some());
            assert!(row.archive_storage);
            assert!(!row.docsrs_version.unwrap().is_empty());
            assert!(!row.rustc_version.unwrap().is_empty());
            assert_eq!(row.build_status.unwrap(), "success");
            assert!(row.source_size > 0);
            assert!(row.documentation_size.unwrap() > 0);

            let mut targets: Vec<String> = row
                .doc_targets
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect();
            targets.sort();

            let runtime = env.runtime();
            let web = runtime.block_on(env.web_app());

            // old rustdoc & source files are gone
            assert!(!storage.exists(&old_rustdoc_file)?);
            assert!(!storage.exists(&old_source_file)?);

            // doc archive exists
            let doc_archive = rustdoc_archive_path(crate_, version);
            assert!(storage.exists(&doc_archive)?, "{}", doc_archive);

            // source archive exists
            let source_archive = source_archive_path(crate_, version);
            assert!(storage.exists(&source_archive)?, "{}", source_archive);

            // default target was built and is accessible
            assert!(storage.exists_in_archive(
                &doc_archive,
                None,
                &format!("{crate_path}/index.html"),
            )?);
            runtime.block_on(web.assert_success(&format!("/{crate_}/{version}/{crate_path}/")))?;

            // source is also packaged
            assert!(storage.exists_in_archive(&source_archive, None, "src/lib.rs",)?);
            runtime.block_on(
                web.assert_success(&format!("/crate/{crate_}/{version}/source/src/lib.rs")),
            )?;
            assert!(!storage.exists_in_archive(
                &doc_archive,
                None,
                &format!("{default_target}/{crate_path}/index.html"),
            )?);

            let default_target_url =
                format!("/{crate_}/{version}/{default_target}/{crate_path}/index.html");
            runtime.block_on(web.assert_redirect(
                &default_target_url,
                &format!("/{crate_}/{version}/{crate_path}/index.html"),
            ))?;

            // Non-dist toolchains only have a single target, and of course
            // if include_default_targets is false we won't have this full list
            // of targets.
            if builder.toolchain.as_dist().is_some() && env.config().include_default_targets {
                assert_eq!(
                    targets,
                    vec![
                        "i686-pc-windows-msvc",
                        "i686-unknown-linux-gnu",
                        "x86_64-apple-darwin",
                        "x86_64-pc-windows-msvc",
                        "x86_64-unknown-linux-gnu",
                    ]
                );

                // other targets too
                for target in DEFAULT_TARGETS {
                    for alg in RUSTDOC_JSON_COMPRESSION_ALGORITHMS {
                        // check if rustdoc json files exist for all targets
                        let path = rustdoc_json_path(
                            crate_,
                            version,
                            target,
                            RustdocJsonFormatVersion::Latest,
                            Some(*alg),
                        );
                        assert!(storage.exists(&path)?);
                        assert!(storage.get_public_access(&path)?);

                        let ext = compression::file_extension_for(*alg);

                        let json_prefix = format!("rustdoc-json/{crate_}/{version}/{target}/");
                        let mut json_files: Vec<_> = storage
                            .list_prefix(&json_prefix)
                            .filter_map(|res| res.ok())
                            .map(|f| f.strip_prefix(&json_prefix).unwrap().to_owned())
                            .collect();
                        json_files.retain(|f| f.ends_with(&format!(".json.{ext}")));
                        json_files.sort();
                        dbg!(&json_files);
                        assert!(
                            json_files[0].starts_with(&format!("empty-library_1.0.0_{target}_"))
                        );

                        assert!(json_files[0].ends_with(&format!(".json.{ext}")));
                        assert_eq!(
                            json_files[1],
                            format!("empty-library_1.0.0_{target}_latest.json.{ext}")
                        );
                    }

                    if target == &default_target {
                        continue;
                    }
                    let target_docs_present = storage.exists_in_archive(
                        &doc_archive,
                        None,
                        &format!("{target}/{crate_path}/index.html"),
                    )?;

                    let target_url =
                        format!("/{crate_}/{version}/{target}/{crate_path}/index.html");

                    assert!(target_docs_present);
                    runtime.block_on(web.assert_success(&target_url))?;

                    assert!(
                        storage
                            .exists(&format!("build-logs/{}/{target}.txt", row.build_id))
                            .unwrap()
                    );
                }
            }

            Ok(())
        })
    }

    #[test]
    #[ignore]
    fn test_collect_metrics() {
        wrapper(|env| {
            let metrics_dir = tempfile::tempdir()?.keep();

            env.override_config(|cfg| {
                cfg.compiler_metrics_collection_path = Some(metrics_dir.clone());
                cfg.include_default_targets = false;
            });

            let crate_ = DUMMY_CRATE_NAME;
            let version = DUMMY_CRATE_VERSION;

            let mut builder = RustwideBuilder::init(env).unwrap();
            builder.update_toolchain()?;
            assert!(
                builder
                    .build_package(crate_, version, PackageKind::CratesIo, true)?
                    .successful
            );

            let metric_files: Vec<_> = fs::read_dir(&metrics_dir)?
                .filter_map(|di| di.ok())
                .map(|di| di.path())
                .collect();

            assert_eq!(metric_files.len(), 1);

            let _: serde_json::Value = serde_json::from_slice(&fs::read(&metric_files[0])?)?;

            Ok(())
        })
    }

    #[test]
    #[ignore]
    fn test_build_binary_crate() {
        wrapper(|env| {
            // some binary crate
            let crate_ = "heater";
            let version = "0.2.3";

            let storage = env.storage();
            let old_rustdoc_file = format!("rustdoc/{crate_}/{version}/some_doc_file");
            let old_source_file = format!("sources/{crate_}/{version}/some_source_file");
            storage.store_one(&old_rustdoc_file, Vec::new())?;
            storage.store_one(&old_source_file, Vec::new())?;

            let mut builder = RustwideBuilder::init(env).unwrap();
            builder.update_toolchain()?;
            assert!(
                !builder
                    .build_package(crate_, version, PackageKind::CratesIo, false)?
                    .successful
            );

            // check release record in the db (default and other targets)
            let row = env.runtime().block_on(async {
                let mut conn = env.async_db().await.async_conn().await;
                sqlx::query!(
                    "SELECT
                        r.rustdoc_status,
                        r.is_library
                    FROM
                        crates as c
                        INNER JOIN releases AS r ON c.id = r.crate_id
                        LEFT OUTER JOIN doc_coverage AS cov ON r.id = cov.release_id
                    WHERE
                        c.name = $1 AND
                        r.version = $2",
                    crate_,
                    version
                )
                .fetch_one(&mut *conn)
                .await
            })?;

            assert_eq!(row.rustdoc_status, Some(false));
            assert_eq!(row.is_library, Some(false));

            // doc archive exists
            let doc_archive = rustdoc_archive_path(crate_, version);
            assert!(!storage.exists(&doc_archive)?);

            // source archive exists
            let source_archive = source_archive_path(crate_, version);
            assert!(storage.exists(&source_archive)?);

            // old rustdoc & source files still exist
            assert!(storage.exists(&old_rustdoc_file)?);
            assert!(storage.exists(&old_source_file)?);

            Ok(())
        })
    }

    #[test]
    #[ignore]
    fn test_failed_build_with_existing_successful_release() {
        wrapper(|env| {
            // rand 0.8.5 fails to build with recent nightly versions
            // https://github.com/rust-lang/docs.rs/issues/26750
            let crate_ = "rand";
            let version = "0.8.5";

            // create a successful release & build in the database
            let release_id = env.runtime().block_on(async {
                let mut conn = env.async_db().await.async_conn().await;
                let crate_id = initialize_crate(&mut conn, crate_).await?;
                let release_id = initialize_release(&mut conn, crate_id, version).await?;
                let build_id = initialize_build(&mut conn, release_id).await?;
                finish_build(
                    &mut conn,
                    build_id,
                    "some-version",
                    "other-version",
                    BuildStatus::Success,
                    None,
                    None,
                )
                .await?;
                finish_release(
                    &mut conn,
                    crate_id,
                    release_id,
                    &MetadataPackage::default(),
                    Path::new("/unknown/"),
                    "x86_64-unknown-linux-gnu",
                    serde_json::Value::Array(vec![]),
                    vec![
                        "i686-pc-windows-msvc".into(),
                        "i686-unknown-linux-gnu".into(),
                        "x86_64-apple-darwin".into(),
                        "x86_64-pc-windows-msvc".into(),
                        "x86_64-unknown-linux-gnu".into(),
                    ],
                    &ReleaseData::default(),
                    true,
                    false,
                    iter::once(CompressionAlgorithm::Bzip2),
                    None,
                    true,
                    42,
                )
                .await?;

                Ok::<_, anyhow::Error>(release_id)
            })?;

            fn check_rustdoc_status(env: &TestEnvironment, rid: ReleaseId) -> Result<()> {
                assert_eq!(
                    env.runtime().block_on(async {
                        let mut conn = env.async_db().await.async_conn().await;
                        sqlx::query_scalar!(
                            "SELECT rustdoc_status FROM releases WHERE id = $1",
                            rid.0
                        )
                        .fetch_one(&mut *conn)
                        .await
                    })?,
                    Some(true)
                );
                Ok(())
            }

            check_rustdoc_status(env, release_id)?;

            let mut builder = RustwideBuilder::init(env).unwrap();
            builder.update_toolchain()?;
            assert!(
                // not successful build
                !builder
                    .build_package(crate_, version, PackageKind::CratesIo, false)?
                    .successful
            );

            check_rustdoc_status(env, release_id)?;
            Ok(())
        });
    }

    #[test_case("scsys-macros", "0.2.6")]
    #[test_case("scsys-derive", "0.2.6")]
    #[test_case("thiserror-impl", "1.0.26")]
    #[ignore]
    fn test_proc_macro(crate_: &str, version: &str) {
        wrapper(|env| {
            let mut builder = RustwideBuilder::init(env).unwrap();
            builder.update_toolchain()?;
            assert!(
                builder
                    .build_package(crate_, version, PackageKind::CratesIo, false)?
                    .successful
            );

            let storage = env.storage();

            // doc archive exists
            let doc_archive = rustdoc_archive_path(crate_, version);
            assert!(storage.exists(&doc_archive)?);

            // source archive exists
            let source_archive = source_archive_path(crate_, version);
            assert!(storage.exists(&source_archive)?);

            Ok(())
        });
    }

    #[test]
    #[ignore]
    fn test_cross_compile_non_host_default() {
        wrapper(|env| {
            let crate_ = "windows-win";
            let version = "2.4.1";
            let mut builder = RustwideBuilder::init(env).unwrap();
            builder.update_toolchain()?;
            if builder.toolchain.as_ci().is_some() {
                return Ok(());
            }
            assert!(
                builder
                    .build_package(crate_, version, PackageKind::CratesIo, false)?
                    .successful
            );

            let storage = env.storage();

            // doc archive exists
            let doc_archive = rustdoc_archive_path(crate_, version);
            assert!(storage.exists(&doc_archive)?, "{}", doc_archive);

            // source archive exists
            let source_archive = source_archive_path(crate_, version);
            assert!(storage.exists(&source_archive)?, "{}", source_archive);

            let target = "x86_64-unknown-linux-gnu";
            let crate_path = crate_.replace('-', "_");
            let target_docs_present = storage.exists_in_archive(
                &doc_archive,
                None,
                &format!("{target}/{crate_path}/index.html"),
            )?;
            assert!(target_docs_present);

            env.runtime().block_on(async {
                let web = env.web_app().await;
                let target_url = format!("/{crate_}/{version}/{target}/{crate_path}/index.html");

                web.assert_success(&target_url).await
            })?;

            Ok(())
        });
    }

    #[test]
    #[ignore]
    fn test_locked_fails_unlocked_needs_new_deps() {
        wrapper(|env| {
            env.override_config(|cfg| cfg.include_default_targets = false);

            // if the corrected dependency of the crate was already downloaded we need to remove it
            remove_cache_files(env, "rand_core", "0.5.1")?;

            // Specific setup required:
            //  * crate has a binary so that it is published with a lockfile
            //  * crate has a library so that it is documented by docs.rs
            //  * crate has an optional dependency
            //  * metadata enables the optional dependency for docs.rs
            //  * `cargo doc` fails with the version of the dependency in the lockfile
            //  * there is a newer version of the dependency available that correctly builds
            let crate_ = "docs_rs_test_incorrect_lockfile";
            let version = "0.1.2";
            let mut builder = RustwideBuilder::init(env).unwrap();
            builder.update_toolchain()?;
            assert!(
                builder
                    .build_package(crate_, version, PackageKind::CratesIo, false)?
                    .successful
            );

            Ok(())
        });
    }

    #[test]
    #[ignore]
    fn test_locked_fails_unlocked_needs_new_unknown_deps() {
        wrapper(|env| {
            env.override_config(|cfg| cfg.include_default_targets = false);

            // if the corrected dependency of the crate was already downloaded we need to remove it
            remove_cache_files(env, "value-bag-sval2", "1.4.1")?;

            // Similar to above, this crate fails to build with the published
            // lockfile, but generating a new working lockfile requires
            // introducing a completely new dependency (not just version) which
            // would not have had its details pulled down from the sparse-index.
            let crate_ = "docs_rs_test_incorrect_lockfile";
            let version = "0.2.0";
            let mut builder = RustwideBuilder::init(env).unwrap();
            builder.update_toolchain()?;
            assert!(
                builder
                    .build_package(crate_, version, PackageKind::CratesIo, false)?
                    .successful
            );

            Ok(())
        });
    }

    #[test]
    #[ignore]
    fn test_rustflags_are_passed_to_build_script() {
        wrapper(|env| {
            let crate_ = "proc-macro2";
            let version = "1.0.95";
            let mut builder = RustwideBuilder::init(env).unwrap();
            builder.update_toolchain()?;
            assert!(
                builder
                    .build_package(crate_, version, PackageKind::CratesIo, false)?
                    .successful
            );
            Ok(())
        });
    }

    #[test]
    #[ignore]
    fn test_sources_are_added_even_for_build_failures_before_build() {
        wrapper(|env| {
            // https://github.com/rust-lang/docs.rs/issues/2523
            // package with invalid cargo metadata.
            // Will succeed in the crate fetch step, so sources are
            // added. Will fail when we try to build.
            let crate_ = "simconnect-sys";
            let version = "0.23.1";
            let mut builder = RustwideBuilder::init(env).unwrap();
            builder.update_toolchain()?;

            // `Result` is `Ok`, but the build-result is `false`
            assert!(
                !builder
                    .build_package(crate_, version, PackageKind::CratesIo, false)?
                    .successful
            );

            // source archive exists
            let source_archive = source_archive_path(crate_, version);
            assert!(
                env.storage().exists(&source_archive)?,
                "archive doesnt exist: {source_archive}"
            );

            Ok(())
        });
    }

    #[test]
    #[ignore]
    fn test_build_failures_before_build() {
        wrapper(|env| {
            // https://github.com/rust-lang/docs.rs/issues/2491
            // package without Cargo.toml, so fails directly in the fetch stage.
            let crate_ = "emheap";
            let version = "0.1.0";
            let mut builder = RustwideBuilder::init(env).unwrap();
            builder.update_toolchain()?;

            // `Result` is `Ok`, but the build-result is `false`
            let summary = builder.build_package(crate_, version, PackageKind::CratesIo, false)?;

            assert!(!summary.successful);
            assert!(summary.should_reattempt);

            let row = env.runtime().block_on(async {
                let mut conn = env.async_db().await.async_conn().await;
                sqlx::query!(
                    r#"SELECT
                       rustc_version,
                       docsrs_version,
                       build_status as "build_status: BuildStatus",
                       errors
                       FROM
                       crates as c
                       INNER JOIN releases as r on c.id = r.crate_id
                       INNER JOIN builds as b on b.rid = r.id
                       WHERE c.name = $1 and r.version = $2"#,
                    crate_,
                    version,
                )
                .fetch_one(&mut *conn)
                .await
            })?;

            assert!(row.rustc_version.is_none());
            assert!(row.docsrs_version.is_none());
            assert_eq!(row.build_status, BuildStatus::Failure);
            assert!(row.errors.unwrap().contains("missing Cargo.toml"));

            Ok(())
        });
    }

    #[test]
    #[ignore]
    fn test_implicit_features_for_optional_dependencies() {
        wrapper(|env| {
            let crate_ = "serde";
            let version = "1.0.152";
            let mut builder = RustwideBuilder::init(env).unwrap();
            builder.update_toolchain()?;
            assert!(
                builder
                    .build_package(crate_, version, PackageKind::CratesIo, false)?
                    .successful
            );

            assert!(
                get_features(env, crate_, version)?
                    .unwrap()
                    .iter()
                    .any(|f| f.name == "serde_derive")
            );

            Ok(())
        });
    }

    #[test]
    #[ignore]
    fn test_no_implicit_features_for_optional_dependencies_with_dep_syntax() {
        wrapper(|env| {
            let crate_ = "stylish-core";
            let version = "0.1.1";
            let mut builder = RustwideBuilder::init(env).unwrap();
            builder.update_toolchain()?;
            assert!(
                builder
                    .build_package(crate_, version, PackageKind::CratesIo, false)?
                    .successful
            );

            assert!(
                !get_features(env, crate_, version)?
                    .unwrap()
                    .iter()
                    .any(|f| f.name == "with_builtin_macros")
            );

            Ok(())
        });
    }

    #[test]
    #[ignore]
    fn test_build_std() {
        wrapper(|env| {
            let mut builder = RustwideBuilder::init(env)?;
            builder.update_toolchain()?;
            assert!(
                builder
                    .build_local_package(Path::new("tests/crates/build-std"))?
                    .successful
            );
            Ok(())
        })
    }

    #[test]
    #[ignore]
    fn test_workspace_reinitialize_at_once() {
        wrapper(|env| {
            let mut builder = RustwideBuilder::init(env)?;
            builder.update_toolchain()?;
            builder.reinitialize_workspace_if_interval_passed(env)?;
            assert!(
                builder
                    .build_local_package(Path::new("tests/crates/build-std"))?
                    .successful
            );
            Ok(())
        })
    }

    #[test]
    #[ignore]
    fn test_workspace_reinitialize_after_interval() {
        use std::thread::sleep;
        use std::time::Duration;
        wrapper(|env: &TestEnvironment| {
            env.override_config(|cfg: &mut Config| {
                cfg.build_workspace_reinitialization_interval = Duration::from_secs(1)
            });
            let mut builder = RustwideBuilder::init(env)?;
            builder.update_toolchain()?;
            assert!(
                builder
                    .build_local_package(Path::new("tests/crates/build-std"))?
                    .successful
            );
            sleep(Duration::from_secs(1));
            builder.reinitialize_workspace_if_interval_passed(env)?;
            assert!(
                builder
                    .build_local_package(Path::new("tests/crates/build-std"))?
                    .successful
            );
            Ok(())
        })
    }

    #[test]
    #[ignore]
    fn test_new_builder_detects_existing_rustc() {
        wrapper(|env: &TestEnvironment| {
            let mut builder = RustwideBuilder::init(env)?;
            builder.update_toolchain()?;
            let old_version = builder.rustc_version()?;
            drop(builder);

            // new builder should detect the existing rustc version from the previous builder
            // (simulating running `update-toolchain` and `build crate` in separate invocations)
            let mut builder = RustwideBuilder::init(env)?;
            assert!(
                builder
                    .build_package(
                        DUMMY_CRATE_NAME,
                        DUMMY_CRATE_VERSION,
                        PackageKind::CratesIo,
                        false
                    )?
                    .successful
            );
            assert_eq!(old_version, builder.rustc_version()?);

            Ok(())
        })
    }

    #[test]
    fn test_read_format_version_from_rustdoc_json() -> Result<()> {
        let buf = serde_json::to_vec(&serde_json::json!({
            "something": "else",
            "format_version": 42
        }))?;

        assert_eq!(
            read_format_version_from_rustdoc_json(&mut io::Cursor::new(buf))?,
            RustdocJsonFormatVersion::Version(42)
        );

        Ok(())
    }
}
