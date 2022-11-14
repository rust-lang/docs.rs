use crate::db::file::add_path_into_database;
use crate::db::{
    add_build_into_database, add_doc_coverage, add_package_into_database,
    add_path_into_remote_archive, update_crate_data_in_database, Pool,
};
use crate::docbuilder::{crates::crates_from_path, Limits};
use crate::error::Result;
use crate::index::api::ReleaseData;
use crate::repositories::RepositoryStatsUpdater;
use crate::storage::{rustdoc_archive_path, source_archive_path};
use crate::utils::{
    copy_dir_all, parse_rustc_version, queue_builder, set_config, CargoMetadata, ConfigName,
};
use crate::RUSTDOC_STATIC_STORAGE_PREFIX;
use crate::{db::blacklist::is_blacklisted, utils::MetadataPackage};
use crate::{Config, Context, Index, Metrics, Storage};
use anyhow::{anyhow, bail, Error};
use docsrs_metadata::{Metadata, DEFAULT_TARGETS, HOST_TARGET};
use failure::Error as FailureError;
use postgres::Client;
use regex::Regex;
use rustwide::cmd::{Command, CommandError, SandboxBuilder, SandboxImage};
use rustwide::logging::{self, LogStorage};
use rustwide::toolchain::ToolchainError;
use rustwide::{AlternativeRegistry, Build, Crate, Toolchain, Workspace, WorkspaceBuilder};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info, warn};

const USER_AGENT: &str = "docs.rs builder (https://github.com/rust-lang/docs.rs)";
const DUMMY_CRATE_NAME: &str = "empty-library";
const DUMMY_CRATE_VERSION: &str = "1.0.0";

pub enum PackageKind<'a> {
    Local(&'a Path),
    CratesIo,
    Registry(&'a str),
}

pub struct RustwideBuilder {
    workspace: Workspace,
    toolchain: Toolchain,
    config: Arc<Config>,
    db: Pool,
    storage: Arc<Storage>,
    metrics: Arc<Metrics>,
    index: Arc<Index>,
    rustc_version: String,
    repository_stats_updater: Arc<RepositoryStatsUpdater>,
    skip_build_if_exists: bool,
}

impl RustwideBuilder {
    pub fn init(context: &dyn Context) -> Result<Self> {
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

        let workspace = builder.init().map_err(FailureError::compat)?;
        workspace
            .purge_all_build_dirs()
            .map_err(FailureError::compat)?;

        // If the toolchain is all hex, assume it references an artifact from
        // CI, for instance an `@bors try` build.
        let re = Regex::new(r"^[a-fA-F0-9]+$").unwrap();
        let toolchain = if re.is_match(&config.toolchain) {
            debug!("using CI build {}", &config.toolchain);
            Toolchain::ci(&config.toolchain, false)
        } else {
            debug!("using toolchain {}", &config.toolchain);
            Toolchain::dist(&config.toolchain)
        };

        Ok(RustwideBuilder {
            workspace,
            toolchain,
            config,
            db: context.pool()?,
            storage: context.storage()?,
            metrics: context.metrics()?,
            index: context.index()?,
            rustc_version: String::new(),
            repository_stats_updater: context.repository_stats_updater()?,
            skip_build_if_exists: false,
        })
    }

    pub fn set_skip_build_if_exists(&mut self, should: bool) {
        self.skip_build_if_exists = should;
    }

    fn prepare_sandbox(&self, limits: &Limits) -> SandboxBuilder {
        SandboxBuilder::new()
            .cpu_limit(self.config.build_cpu_limit.map(|limit| limit as f32))
            .memory_limit(Some(limits.memory()))
            .enable_networking(limits.networking())
    }

    pub fn purge_caches(&self) -> Result<()> {
        self.workspace
            .purge_all_caches()
            .map_err(FailureError::compat)?;
        Ok(())
    }

    pub fn update_toolchain(&mut self) -> Result<bool> {
        // For CI builds, a lot of the normal update_toolchain things don't apply.
        // CI builds are only for one platform (https://forge.rust-lang.org/infra/docs/rustc-ci.html#try-builds)
        // so we only try installing for the current platform. If that's not a match,
        // for instance if we're running on macOS or Windows, this will error.
        // Also, detecting the rustc version relies on calling rustc through rustup with the
        // +channel argument, but the +channel argument doesn't work for CI builds. So
        // we fake the rustc version and install from scratch every time since we can't detect
        // the already-installed rustc version.
        if let Some(ci) = self.toolchain.as_ci() {
            self.toolchain
                .install(&self.workspace)
                .map_err(FailureError::compat)?;
            self.rustc_version = format!("rustc 1.9999.0-nightly ({} 2999-12-29)", ci.sha());
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
                    return Err(err.compat().into());
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
                self.toolchain
                    .remove_target(&self.workspace, &target)
                    .map_err(FailureError::compat)?;
            }
        }

        self.toolchain
            .install(&self.workspace)
            .map_err(FailureError::compat)?;

        for target in &targets_to_install {
            self.toolchain
                .add_target(&self.workspace, target)
                .map_err(FailureError::compat)?;
        }
        // NOTE: rustup will automatically refuse to update the toolchain
        // if `rustfmt` is not available in the newer version
        // NOTE: this ignores the error so that you can still run a build without rustfmt.
        // This should only happen if you run a build for the first time when rustfmt isn't available.
        if let Err(err) = self.toolchain.add_component(&self.workspace, "rustfmt") {
            warn!("failed to install rustfmt: {}", err);
            info!("continuing anyway, since this must be the first build");
        }

        self.rustc_version = self.detect_rustc_version()?;

        let has_changed = old_version.as_deref() != Some(&self.rustc_version);
        if has_changed {
            self.add_essential_files()?;
        }
        Ok(has_changed)
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

    pub fn add_essential_files(&mut self) -> Result<()> {
        let rustc_version = parse_rustc_version(&self.rustc_version)?;

        info!("building a dummy crate to get essential files");

        let mut conn = self.db.get()?;
        let limits = Limits::for_crate(&mut conn, DUMMY_CRATE_NAME)?;

        let mut build_dir = self
            .workspace
            .build_dir(&format!("essential-files-{}", rustc_version));
        build_dir.purge().map_err(FailureError::compat)?;

        // This is an empty library crate that is supposed to always build.
        let krate = Crate::crates_io(DUMMY_CRATE_NAME, DUMMY_CRATE_VERSION);
        krate.fetch(&self.workspace).map_err(FailureError::compat)?;

        build_dir
            .build(&self.toolchain, &krate, self.prepare_sandbox(&limits))
            .run(|build| {
                (|| -> Result<()> {
                    let metadata = Metadata::from_crate_root(&build.host_source_dir())?;

                    let res =
                        self.execute_build(HOST_TARGET, true, build, &limits, &metadata, true)?;
                    if !res.result.successful {
                        bail!("failed to build dummy crate for {}", self.rustc_version);
                    }

                    info!("copying essential files for {}", self.rustc_version);
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
                        add_path_into_database(
                            &self.storage,
                            RUSTDOC_STATIC_STORAGE_PREFIX,
                            &static_files,
                        )?;
                    } else {
                        add_path_into_database(
                            &self.storage,
                            RUSTDOC_STATIC_STORAGE_PREFIX,
                            &dest,
                        )?;
                    }

                    set_config(
                        &mut conn,
                        ConfigName::RustcVersion,
                        self.rustc_version.clone(),
                    )?;
                    Ok(())
                })()
                .map_err(|e| failure::Error::from_boxed_compat(e.into()))
            })
            .map_err(|e| e.compat())?;

        build_dir.purge().map_err(FailureError::compat)?;
        krate
            .purge_from_cache(&self.workspace)
            .map_err(FailureError::compat)?;
        Ok(())
    }

    pub fn build_world(&mut self) -> Result<()> {
        crates_from_path(
            &self.config.registry_index_path.clone(),
            &mut |name, version| {
                let registry_url = self.config.registry_url.clone();
                let package_kind = registry_url
                    .as_ref()
                    .map(|r| PackageKind::Registry(r.as_str()))
                    .unwrap_or(PackageKind::CratesIo);
                if let Err(err) = self.build_package(name, version, package_kind) {
                    warn!("failed to build package {} {}: {}", name, version, err);
                }
            },
        )
    }

    pub fn build_local_package(&mut self, path: &Path) -> Result<bool> {
        self.update_toolchain()?;
        let metadata =
            CargoMetadata::load(&self.workspace, &self.toolchain, path).map_err(|err| {
                err.context(format!("failed to load local package {}", path.display()))
            })?;
        let package = metadata.root();
        self.build_package(&package.name, &package.version, PackageKind::Local(path))
    }

    pub fn build_package(
        &mut self,
        name: &str,
        version: &str,
        kind: PackageKind<'_>,
    ) -> Result<bool> {
        let mut conn = self.db.get()?;

        if !self.should_build(&mut conn, name, version)? {
            return Ok(false);
        }

        self.update_toolchain()?;

        info!("building package {} {}", name, version);

        if is_blacklisted(&mut conn, name)? {
            info!("skipping build of {}, crate has been blacklisted", name);
            return Ok(false);
        }

        let limits = Limits::for_crate(&mut conn, name)?;
        #[cfg(target_os = "linux")]
        if !self.config.disable_memory_limit {
            use anyhow::Context;
            let mem_info = procfs::Meminfo::new().context("failed to read /proc/meminfo")?;
            let available = mem_info
                .mem_available
                .expect("kernel version too old for determining memory limit");
            if limits.memory() as u64 > available {
                bail!("not enough memory to build {} {}: needed {} MiB, have {} MiB\nhelp: set DOCSRS_DISABLE_MEMORY_LIMIT=true to force a build",
                    name, version, limits.memory() / 1024 / 1024, available / 1024 / 1024
                );
            } else {
                debug!(
                    "had enough memory: {} MiB <= {} MiB",
                    limits.memory() / 1024 / 1024,
                    available / 1024 / 1024
                );
            }
        }

        let mut build_dir = self.workspace.build_dir(&format!("{}-{}", name, version));
        build_dir.purge().map_err(FailureError::compat)?;

        let krate = match kind {
            PackageKind::Local(path) => Crate::local(path),
            PackageKind::CratesIo => Crate::crates_io(name, version),
            PackageKind::Registry(registry) => {
                Crate::registry(AlternativeRegistry::new(registry), name, version)
            }
        };
        krate.fetch(&self.workspace).map_err(FailureError::compat)?;

        let local_storage = tempfile::Builder::new()
            .prefix(queue_builder::TEMPDIR_PREFIX)
            .tempdir()?;

        let successful = build_dir
            .build(&self.toolchain, &krate, self.prepare_sandbox(&limits))
            .run(|build| {
                (|| -> Result<bool> {
                    use docsrs_metadata::BuildTargets;

                    let mut has_docs = false;
                    let mut successful_targets = Vec::new();
                    let metadata = Metadata::from_crate_root(&build.host_source_dir())?;
                    let BuildTargets {
                        default_target,
                        other_targets,
                    } = metadata.targets(self.config.include_default_targets);

                    // Perform an initial build
                    let mut res =
                        self.execute_build(default_target, true, build, &limits, &metadata, false)?;

                    // If the build fails with the lockfile given, try using only the dependencies listed in Cargo.toml.
                    let cargo_lock = build.host_source_dir().join("Cargo.lock");
                    if !res.result.successful && cargo_lock.exists() {
                        info!("removing lockfile and reattempting build");
                        std::fs::remove_file(cargo_lock)?;
                        Command::new(&self.workspace, self.toolchain.cargo())
                            .cd(build.host_source_dir())
                            .args(&["generate-lockfile", "-Zno-index-update"])
                            .run()?;
                        Command::new(&self.workspace, self.toolchain.cargo())
                            .cd(build.host_source_dir())
                            .args(&["fetch", "--locked"])
                            .run()?;
                        res = self.execute_build(
                            default_target,
                            true,
                            build,
                            &limits,
                            &metadata,
                            false,
                        )?;
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

                    let mut algs = HashSet::new();
                    if has_docs {
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
                            self.build_target(
                                target,
                                build,
                                &limits,
                                local_storage.path(),
                                &mut successful_targets,
                                &metadata,
                            )?;
                        }
                        let (_, new_alg) = add_path_into_remote_archive(
                            &self.storage,
                            &rustdoc_archive_path(name, version),
                            local_storage.path(),
                            true,
                        )?;
                        algs.insert(new_alg);
                    };

                    // Store the sources even if the build fails
                    debug!("adding sources into database");
                    let files_list = {
                        let (files_list, new_alg) = add_path_into_remote_archive(
                            &self.storage,
                            &source_archive_path(name, version),
                            build.host_source_dir(),
                            false,
                        )?;
                        algs.insert(new_alg);
                        files_list
                    };

                    let has_examples = build.host_source_dir().join("examples").is_dir();
                    if res.result.successful {
                        self.metrics.successful_builds.inc();
                    } else if res.cargo_metadata.root().is_library() {
                        self.metrics.failed_builds.inc();
                    } else {
                        self.metrics.non_library_builds.inc();
                    }

                    let release_data = match self.index.api().get_release_data(name, version) {
                        Ok(data) => data,
                        Err(err) => {
                            warn!("{:#?}", err);
                            ReleaseData::default()
                        }
                    };

                    let cargo_metadata = res.cargo_metadata.root();
                    let repository = self.get_repo(cargo_metadata)?;

                    let release_id = add_package_into_database(
                        &mut conn,
                        cargo_metadata,
                        &build.host_source_dir(),
                        &res.result,
                        &res.target,
                        files_list,
                        successful_targets,
                        &release_data,
                        has_docs,
                        has_examples,
                        algs,
                        repository,
                        true,
                    )?;

                    if let Some(doc_coverage) = res.doc_coverage {
                        add_doc_coverage(&mut conn, release_id, doc_coverage)?;
                    }

                    let build_id = add_build_into_database(&mut conn, release_id, &res.result)?;
                    let build_log_path = format!("build-logs/{}/{}.txt", build_id, default_target);
                    self.storage.store_one(build_log_path, res.build_log)?;

                    // Some crates.io crate data is mutable, so we proactively update it during a release
                    match self.index.api().get_crate_data(name) {
                        Ok(crate_data) => {
                            update_crate_data_in_database(&mut conn, name, &crate_data)?
                        }
                        Err(err) => warn!("{:#?}", err),
                    }

                    if res.result.successful {
                        // delete eventually existing files from pre-archive storage.
                        // we're doing this in the end so eventual problems in the build
                        // won't lead to non-existing docs.
                        for prefix in &["rustdoc", "sources"] {
                            let prefix = format!("{}/{}/{}/", prefix, name, version);
                            debug!("cleaning old storage folder {}", prefix);
                            self.storage.delete_prefix(&prefix)?;
                        }
                    }

                    Ok(res.result.successful)
                })()
                .map_err(|e| failure::Error::from_boxed_compat(e.into()))
            })
            .map_err(|e| e.compat())?;

        build_dir.purge().map_err(FailureError::compat)?;
        krate
            .purge_from_cache(&self.workspace)
            .map_err(FailureError::compat)?;
        local_storage.close()?;
        Ok(successful)
    }

    fn build_target(
        &self,
        target: &str,
        build: &Build,
        limits: &Limits,
        local_storage: &Path,
        successful_targets: &mut Vec<String>,
        metadata: &Metadata,
    ) -> Result<()> {
        let target_res = self.execute_build(target, false, build, limits, metadata, false)?;
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
        Ok(())
    }

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

        self.prepare_command(build, target, metadata, limits, rustdoc_flags)?
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
            .log_output(false)
            .run()?;

        Ok(
            if coverage.total_items == 0 && coverage.documented_items == 0 {
                None
            } else {
                Some(coverage)
            },
        )
    }

    fn execute_build(
        &self,
        target: &str,
        is_default_target: bool,
        build: &Build,
        limits: &Limits,
        metadata: &Metadata,
        create_essential_files: bool,
    ) -> Result<FullBuildResult> {
        let cargo_metadata =
            CargoMetadata::load(&self.workspace, &self.toolchain, &build.host_source_dir())?;

        let mut rustdoc_flags = vec![if create_essential_files {
            "--emit=unversioned-shared-resources,toolchain-shared-resources"
        } else {
            "--emit=invocation-specific"
        }
        .to_string()];
        rustdoc_flags.extend(vec![
            "--resource-suffix".to_string(),
            format!("-{}", parse_rustc_version(&self.rustc_version)?),
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

        let successful = logging::capture(&storage, || {
            self.prepare_command(build, target, metadata, limits, rustdoc_flags)
                .and_then(|command| command.run().map_err(Error::from))
                .is_ok()
        });

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
                rustc_version: self.rustc_version.clone(),
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
    ) -> Result<Command<'ws, 'pl>> {
        // If the explicit target is not a tier one target, we need to install it.
        if !docsrs_metadata::DEFAULT_TARGETS.contains(&target) {
            // This is a no-op if the target is already installed.
            self.toolchain
                .add_target(&self.workspace, target)
                .map_err(FailureError::compat)?;
        }

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
                r#"--config=doc.extern-map.registries.crates-io="https://docs.rs/{{pkg_name}}/{{version}}/{}""#,
                target
            ),
        ];
        if let Some(cpu_limit) = self.config.build_cpu_limit {
            cargo_args.push(format!("-j{}", cpu_limit));
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
            "--disable-per-crate-search",
            "--extern-html-root-takes-precedence",
        ];

        rustdoc_flags_extras.extend(UNCONDITIONAL_ARGS.iter().map(|&s| s.to_owned()));
        let cargo_args = metadata.cargo_args(&cargo_args, &rustdoc_flags_extras);

        let mut command = build
            .cargo()
            .timeout(Some(limits.timeout()))
            .no_output_timeout(None);

        for (key, val) in metadata.environment_variables() {
            command = command.env(key, val);
        }

        Ok(command.args(&cargo_args))
    }

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

    fn should_build(&self, conn: &mut Client, name: &str, version: &str) -> Result<bool> {
        if self.skip_build_if_exists {
            // Check whether no successful builds are present in the database.
            Ok(conn
                .query(
                    "SELECT 1 FROM crates, releases, builds
                     WHERE crates.id = releases.crate_id AND releases.id = builds.rid
                       AND crates.name = $1 AND releases.version = $2
                       AND builds.build_status = TRUE;",
                    &[&name, &version],
                )?
                .is_empty())
        } else {
            Ok(true)
        }
    }

    fn get_repo(&self, metadata: &MetadataPackage) -> Result<Option<i32>> {
        self.repository_stats_updater
            .load_repository(metadata)
            .map_err(Into::into)
    }
}

struct FullBuildResult {
    result: BuildResult,
    target: String,
    cargo_metadata: CargoMetadata,
    doc_coverage: Option<DocCoverage>,
    build_log: String,
}

#[derive(Clone, Copy)]
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

pub(crate) struct BuildResult {
    pub(crate) rustc_version: String,
    pub(crate) docsrs_version: String,
    pub(crate) successful: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::{assert_redirect, assert_success, wrapper};
    use serde_json::Value;

    #[test]
    #[ignore]
    fn test_build_crate() {
        wrapper(|env| {
            let crate_ = DUMMY_CRATE_NAME;
            let crate_path = crate_.replace('-', "_");
            let version = DUMMY_CRATE_VERSION;
            let default_target = "x86_64-unknown-linux-gnu";

            let storage = env.storage();
            let old_rustdoc_file = format!("rustdoc/{}/{}/some_doc_file", crate_, version);
            let old_source_file = format!("sources/{}/{}/some_source_file", crate_, version);
            storage.store_one(&old_rustdoc_file, Vec::new())?;
            storage.store_one(&old_source_file, Vec::new())?;

            let mut builder = RustwideBuilder::init(env).unwrap();
            assert!(builder.build_package(crate_, version, PackageKind::CratesIo)?);

            // check release record in the db (default and other targets)
            let mut conn = env.db().conn();
            let rows = conn
                .query(
                    "SELECT
                        r.rustdoc_status,
                        r.default_target,
                        r.doc_targets,
                        r.archive_storage,
                        cov.total_items
                    FROM
                        crates as c
                        INNER JOIN releases AS r ON c.id = r.crate_id
                        LEFT OUTER JOIN doc_coverage AS cov ON r.id = cov.release_id
                    WHERE
                        c.name = $1 AND
                        r.version = $2",
                    &[&crate_, &version],
                )
                .unwrap();
            let row = rows.get(0).unwrap();

            assert!(row.get::<_, bool>("rustdoc_status"));
            assert_eq!(row.get::<_, String>("default_target"), default_target);
            assert!(row.get::<_, Option<i32>>("total_items").is_some());
            assert!(row.get::<_, bool>("archive_storage"));

            let mut targets: Vec<String> = row
                .get::<_, Value>("doc_targets")
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_owned())
                .collect();
            targets.sort();
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

            let web = env.frontend();

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
            assert!(storage.exists_in_archive(&doc_archive, &format!("{}/index.html", crate_path))?);
            assert_success(&format!("/{}/{}/{}", crate_, version, crate_path), web)?;

            // source is also packaged
            assert!(storage.exists_in_archive(&source_archive, "src/lib.rs")?);
            assert_success(
                &format!("/crate/{}/{}/source/src/lib.rs", crate_, version),
                web,
            )?;

            // other targets too
            for target in DEFAULT_TARGETS {
                let target_docs_present = storage.exists_in_archive(
                    &doc_archive,
                    &format!("{}/{}/index.html", target, crate_path),
                )?;

                let target_url = format!(
                    "/{}/{}/{}/{}/index.html",
                    crate_, version, target, crate_path
                );

                if target == &default_target {
                    assert!(!target_docs_present);
                    assert_redirect(
                        &target_url,
                        &format!("/{}/{}/{}/index.html", crate_, version, crate_path),
                        web,
                    )?;
                } else {
                    assert!(target_docs_present);
                    assert_success(&target_url, web)?;
                }
            }

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
            let old_rustdoc_file = format!("rustdoc/{}/{}/some_doc_file", crate_, version);
            let old_source_file = format!("sources/{}/{}/some_source_file", crate_, version);
            storage.store_one(&old_rustdoc_file, Vec::new())?;
            storage.store_one(&old_source_file, Vec::new())?;

            let mut builder = RustwideBuilder::init(env).unwrap();
            assert!(!builder.build_package(crate_, version, PackageKind::CratesIo)?);

            // check release record in the db (default and other targets)
            let mut conn = env.db().conn();
            let rows = conn
                .query(
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
                    &[&crate_, &version],
                )
                .unwrap();
            let row = rows.get(0).unwrap();

            assert!(!row.get::<_, bool>("rustdoc_status"));
            assert!(!row.get::<_, bool>("is_library"));

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
    fn test_proc_macro() {
        wrapper(|env| {
            let crate_ = "thiserror-impl";
            let version = "1.0.26";
            let mut builder = RustwideBuilder::init(env).unwrap();
            assert!(builder.build_package(crate_, version, PackageKind::CratesIo)?);

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
            assert!(builder.build_package(crate_, version, PackageKind::CratesIo)?);

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
                &format!("{}/{}/index.html", target, crate_path),
            )?;

            let web = env.frontend();
            let target_url = format!(
                "/{}/{}/{}/{}/index.html",
                crate_, version, target, crate_path
            );

            assert!(target_docs_present);
            assert_success(&target_url, web)?;

            Ok(())
        });
    }

    #[test]
    #[ignore]
    fn test_locked_fails_unlocked_needs_new_deps() {
        wrapper(|env| {
            env.override_config(|cfg| cfg.include_default_targets = false);

            // if the corrected dependency of the crate was already downloaded we need to remove it
            let crate_file = env.config().rustwide_workspace.join(
                "cargo-home/registry/cache/github.com-1ecc6299db9ec823/rand_core-0.5.1.crate",
            );
            let src_dir = env
                .config()
                .rustwide_workspace
                .join("cargo-home/registry/src/github.com-1ecc6299db9ec823/rand_core-0.5.1");

            if crate_file.exists() {
                info!("deleting {}", crate_file.display());
                std::fs::remove_file(crate_file)?;
            }
            if src_dir.exists() {
                info!("deleting {}", src_dir.display());
                std::fs::remove_dir_all(src_dir)?;
            }

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
            assert!(builder.build_package(crate_, version, PackageKind::CratesIo)?);

            Ok(())
        });
    }

    #[test]
    #[ignore]
    fn test_rustflags_are_passed_to_build_script() {
        wrapper(|env| {
            let crate_ = "proc-macro2";
            let version = "1.0.33";
            let mut builder = RustwideBuilder::init(env).unwrap();
            assert!(builder.build_package(crate_, version, PackageKind::CratesIo)?);
            Ok(())
        });
    }
}
