use super::DocBuilder;
use db::blacklist::is_blacklisted;
use db::file::add_path_into_database;
use db::{add_build_into_database, add_package_into_database, connect_db, CratesIoData};
use docbuilder::{crates::crates_from_path, Limits};
use error::Result;
use failure::ResultExt;
use log::LevelFilter;
use postgres::Connection;
use rustc_serialize::json::ToJson;
use rustwide::cmd::{Command, SandboxBuilder};
use rustwide::logging::{self, LogStorage};
use rustwide::toolchain::{Toolchain, ToolchainError};
use rustwide::{Build, Crate, Workspace, WorkspaceBuilder};
use std::borrow::Cow;
use std::collections::HashSet;
use std::path::Path;
use utils::{copy_doc_dir, parse_rustc_version, CargoMetadata};
use super::Metadata;

const USER_AGENT: &str = "docs.rs builder (https://github.com/rust-lang/docs.rs)";
const DEFAULT_RUSTWIDE_WORKSPACE: &str = ".rustwide";

// It is crucial that this be the same as the host that `docs.rs` is being run on.
// Other values may cause strange and hard-to-debug errors.
// TODO: use `TARGET` instead? I think `TARGET` is only set for build scripts, though.
pub(super) const HOST_TARGET: &str = "x86_64-unknown-linux-gnu";
pub(super) const TARGETS: &[&str] = &[
    "i686-pc-windows-msvc",
    "i686-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
];

const ESSENTIAL_FILES_VERSIONED: &[&str] = &[
    "brush.svg",
    "wheel.svg",
    "down-arrow.svg",
    "dark.css",
    "light.css",
    "main.js",
    "normalize.css",
    "rustdoc.css",
    "settings.css",
    "settings.js",
    "storage.js",
    "theme.js",
    "source-script.js",
    "noscript.css",
    "rust-logo.png",
];
const ESSENTIAL_FILES_UNVERSIONED: &[&str] = &[
    "FiraSans-Medium.woff",
    "FiraSans-Regular.woff",
    "SourceCodePro-Regular.woff",
    "SourceCodePro-Semibold.woff",
    "SourceSerifPro-Bold.ttf.woff",
    "SourceSerifPro-Regular.ttf.woff",
    "SourceSerifPro-It.ttf.woff",
];

const DUMMY_CRATE_NAME: &str = "acme-client";
const DUMMY_CRATE_VERSION: &str = "0.0.0";

pub struct RustwideBuilder {
    workspace: Workspace,
    toolchain: Toolchain,
    rustc_version: String,
}

impl RustwideBuilder {
    pub fn init() -> Result<Self> {
        use rustwide::cmd::SandboxImage;
        let env_workspace_path = ::std::env::var("CRATESFYI_RUSTWIDE_WORKSPACE");
        let workspace_path = env_workspace_path
            .as_ref()
            .map(|v| v.as_str())
            .unwrap_or(DEFAULT_RUSTWIDE_WORKSPACE);
        let is_docker = std::env::var("DOCS_RS_DOCKER")
            .map(|s| s == "true")
            .unwrap_or(false);
        let mut builder = WorkspaceBuilder::new(Path::new(workspace_path), USER_AGENT)
            .running_inside_docker(is_docker);
        if let Ok(custom_image) = std::env::var("DOCS_RS_LOCAL_DOCKER_IMAGE") {
            builder = builder.sandbox_image(SandboxImage::local(&custom_image)?);
        }

        let workspace = builder.init()?;
        workspace.purge_all_build_dirs()?;

        let toolchain_name = std::env::var("CRATESFYI_TOOLCHAIN")
            .map(|t| Cow::Owned(t))
            .unwrap_or_else(|_| Cow::Borrowed("nightly"));

        let toolchain = Toolchain::dist(&toolchain_name);

        Ok(RustwideBuilder {
            workspace,
            toolchain,
            rustc_version: String::new(),
        })
    }

    pub fn update_toolchain(&mut self) -> Result<()> {
        // Ignore errors if detection fails.
        let old_version = self.detect_rustc_version().ok();

        let mut targets_to_install = TARGETS
            .iter()
            .map(|t| t.to_string())
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

        self.rustc_version = self.detect_rustc_version()?;
        if old_version.as_ref().map(|s| s.as_str()) != Some(&self.rustc_version) {
            self.add_essential_files()?;
        }

        Ok(())
    }

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
            Err(::failure::err_msg(
                "invalid output returned by `rustc --version`",
            ))
        }
    }

    pub fn add_essential_files(&mut self) -> Result<()> {
        self.rustc_version = self.detect_rustc_version()?;
        let rustc_version = parse_rustc_version(&self.rustc_version)?;

        info!("building a dummy crate to get essential files");

        let conn = connect_db()?;
        let limits = Limits::for_crate(&conn, DUMMY_CRATE_NAME)?;

        let mut build_dir = self
            .workspace
            .build_dir(&format!("essential-files-{}", rustc_version));
        build_dir.purge()?;

        // acme-client-0.0.0 is an empty library crate and it will always build
        let krate = Crate::crates_io(DUMMY_CRATE_NAME, DUMMY_CRATE_VERSION);
        krate.fetch(&self.workspace)?;

        let sandbox = SandboxBuilder::new()
            .memory_limit(Some(limits.memory()))
            .enable_networking(limits.networking());

        build_dir
            .build(&self.toolchain, &krate, sandbox)
            .run(|build| {
                let metadata = Metadata::from_source_dir(&build.host_source_dir())?;

                let res = self.execute_build(HOST_TARGET, true, build, &limits, &metadata)?;
                if !res.result.successful {
                    bail!("failed to build dummy crate for {}", self.rustc_version);
                }

                info!("copying essential files for {}", self.rustc_version);
                let source = build.host_target_dir().join("doc");
                let dest = ::tempdir::TempDir::new("essential-files")?;

                let files = ESSENTIAL_FILES_VERSIONED
                    .iter()
                    .map(|f| (f, true))
                    .chain(ESSENTIAL_FILES_UNVERSIONED.iter().map(|f| (f, false)));
                for (file, versioned) in files {
                    let segments = file.rsplitn(2, '.').collect::<Vec<_>>();
                    let file_name = if versioned {
                        format!("{}-{}.{}", segments[1], rustc_version, segments[0])
                    } else {
                        file.to_string()
                    };
                    let source_path = source.join(&file_name);
                    let dest_path = dest.path().join(&file_name);
                    ::std::fs::copy(&source_path, &dest_path).with_context(|_| {
                        format!(
                            "couldn't copy '{}' to '{}'",
                            source_path.display(),
                            dest_path.display()
                        )
                    })?;
                }

                add_path_into_database(&conn, "", &dest)?;
                conn.query(
                    "INSERT INTO config (name, value) VALUES ('rustc_version', $1) \
                     ON CONFLICT (name) DO UPDATE SET value = $1;",
                    &[&self.rustc_version.to_json()],
                )?;

                Ok(())
            })?;

        build_dir.purge()?;
        krate.purge_from_cache(&self.workspace)?;
        Ok(())
    }

    pub fn build_world(&mut self, doc_builder: &mut DocBuilder) -> Result<()> {
        let mut count = 0;
        crates_from_path(
            &doc_builder.options().crates_io_index_path.clone(),
            &mut |name, version| {
                match self.build_package(doc_builder, name, version, None) {
                    Ok(status) => {
                        count += 1;
                        if status && count % 10 == 0 {
                            let _ = doc_builder.save_cache();
                        }
                    }
                    Err(err) => warn!("failed to build package {} {}: {}", name, version, err),
                }
                doc_builder.add_to_cache(name, version);
            },
        )
    }

    pub fn build_local_package(
        &mut self,
        doc_builder: &mut DocBuilder,
        path: &Path,
    ) -> Result<bool> {
        self.update_toolchain()?;
        let metadata = CargoMetadata::load(&self.workspace, &self.toolchain, path)
            .map_err(|err| err.context(format!("failed to load local package {}", path.display())))?;
        let package = metadata.root();
        self.build_package(doc_builder, &package.name, &package.version, Some(path))
    }

    pub fn build_package(
        &mut self,
        doc_builder: &mut DocBuilder,
        name: &str,
        version: &str,
        local: Option<&Path>,
    ) -> Result<bool> {
        if !doc_builder.should_build(name, version) {
            return Ok(false);
        }

        self.update_toolchain()?;

        info!("building package {} {}", name, version);

        let conn = connect_db()?;

        if is_blacklisted(&conn, name)? {
            info!("skipping build of {}, crate has been blacklisted", name);
            return Ok(false);
        }

        let limits = Limits::for_crate(&conn, name)?;

        let mut build_dir = self.workspace.build_dir(&format!("{}-{}", name, version));
        build_dir.purge()?;

        let krate = if let Some(path) = local {
            Crate::local(path)
        } else {
            Crate::crates_io(name, version)
        };
        krate.fetch(&self.workspace)?;

        let sandbox = SandboxBuilder::new()
            .memory_limit(Some(limits.memory()))
            .enable_networking(limits.networking());

        let local_storage = ::tempdir::TempDir::new("docsrs-docs")?;

        let res = build_dir
            .build(&self.toolchain, &krate, sandbox)
            .run(|build| {
                use docbuilder::metadata::BuildTargets;

                let mut files_list = None;
                let mut has_docs = false;
                let mut successful_targets = Vec::new();
                let metadata = Metadata::from_source_dir(&build.host_source_dir())?;
                let BuildTargets { default_target, other_targets } = metadata.targets();

                // Do an initial build and then copy the sources in the database
                let res = self.execute_build(default_target, true, &build, &limits, &metadata)?;
                if res.result.successful {
                    debug!("adding sources into database");
                    let prefix = format!("sources/{}/{}", name, version);
                    files_list = Some(add_path_into_database(
                        &conn,
                        &prefix,
                        build.host_source_dir(),
                    )?);

                    if let Some(name) = res.cargo_metadata.root().library_name() {
                        let host_target = build.host_target_dir();
                        has_docs = host_target.join("doc").join(name).is_dir();
                    }
                }

                if has_docs {
                    debug!("adding documentation for the default target to the database");
                    self.copy_docs(
                        &build.host_target_dir(),
                        local_storage.path(),
                        "",
                        true,
                    )?;

                    successful_targets.push(res.target.clone());

                    // Then build the documentation for all the targets
                    // Limit the number of targets so that no one can try to build all 200000 possible targets
                    for target in other_targets.into_iter().take(limits.targets()) {
                        debug!("building package {} {} for {}", name, version, target);
                        self.build_target(
                            target,
                            &build,
                            &limits,
                            &local_storage.path(),
                            &mut successful_targets,
                            &metadata,
                        )?;
                    }
                    self.upload_docs(&conn, name, version, local_storage.path())?;
                }

                let has_examples = build.host_source_dir().join("examples").is_dir();
                if res.result.successful {
                    ::web::metrics::SUCCESSFUL_BUILDS.inc();
                } else if res.cargo_metadata.root().is_library() {
                    ::web::metrics::FAILED_BUILDS.inc();
                } else {
                    ::web::metrics::NON_LIBRARY_BUILDS.inc();
                }
                let release_id = add_package_into_database(
                    &conn,
                    res.cargo_metadata.root(),
                    &build.host_source_dir(),
                    &res.result,
                    &res.target,
                    files_list,
                    successful_targets,
                    &CratesIoData::get_from_network(res.cargo_metadata.root())?,
                    has_docs,
                    has_examples,
                )?;
                add_build_into_database(&conn, &release_id, &res.result)?;

                doc_builder.add_to_cache(name, version);
                Ok(res)
            })?;

        build_dir.purge()?;
        krate.purge_from_cache(&self.workspace)?;
        local_storage.close()?;
        Ok(res.result.successful)
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
        let target_res = self.execute_build(target, false, build, limits, metadata)?;
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

    fn execute_build(
        &self,
        target: &str,
        is_default_target: bool,
        build: &Build,
        limits: &Limits,
        metadata: &Metadata,
    ) -> Result<FullBuildResult> {
        let cargo_metadata =
            CargoMetadata::load(&self.workspace, &self.toolchain, &build.host_source_dir())?;

        let mut rustdoc_flags: Vec<String> = vec![
            "-Z".to_string(),
            "unstable-options".to_string(),
            "--resource-suffix".to_string(),
            format!("-{}", parse_rustc_version(&self.rustc_version)?),
            "--static-root-path".to_string(),
            "/".to_string(),
            "--cap-lints".to_string(),
            "warn".to_string(),
        ];
        for dep in &cargo_metadata.root_dependencies() {
            rustdoc_flags.push("--extern-html-root-url".to_string());
            rustdoc_flags.push(format!(
                "{}=https://docs.rs/{}/{}",
                dep.name.replace("-", "_"),
                dep.name,
                dep.version
            ));
        }
        if let Some(package_rustdoc_args) = &metadata.rustdoc_args {
            rustdoc_flags.append(&mut package_rustdoc_args.iter().map(|s| s.to_owned()).collect());
        }
        let mut cargo_args = vec!["doc".to_owned(), "--lib".to_owned(), "--no-deps".to_owned()];
        if target != HOST_TARGET {
            // If the explicit target is not a tier one target, we need to install it.
            if !TARGETS.contains(&target) {
                // This is a no-op if the target is already installed.
                self.toolchain.add_target(&self.workspace, target)?;
            }
            cargo_args.push("--target".to_owned());
            cargo_args.push(target.to_owned());
        };
        if let Some(features) = &metadata.features {
            cargo_args.push("--features".to_owned());
            cargo_args.push(features.join(" "));
        }
        if metadata.all_features {
            cargo_args.push("--all-features".to_owned());
        }
        if metadata.no_default_features {
            cargo_args.push("--no-default-features".to_owned());
        }

        let mut storage = LogStorage::new(LevelFilter::Info);
        storage.set_max_size(limits.max_log_size());

        let successful = logging::capture(&storage, || {
            build
                .cargo()
                .timeout(Some(limits.timeout()))
                .no_output_timeout(None)
                .env(
                    "RUSTFLAGS",
                    metadata
                        .rustc_args
                        .as_ref()
                        .map(|args| args.join(" "))
                        .unwrap_or_default(),
                )
                .env("RUSTDOCFLAGS", rustdoc_flags.join(" "))
                .args(&cargo_args)
                .run()
                .is_ok()
        });
        // If we're passed a default_target which requires a cross-compile,
        // cargo will put the output in `target/<target>/doc`.
        // However, if this is the default build, we don't want it there,
        // we want it in `target/doc`.
        if target != HOST_TARGET && is_default_target {
            // mv target/$target/doc target/doc
            let target_dir = build.host_target_dir();
            let old_dir = target_dir.join(target).join("doc");
            let new_dir = target_dir.join("doc");
            debug!("rename {} to {}", old_dir.display(), new_dir.display());
            std::fs::rename(old_dir, new_dir)?;
        }

        Ok(FullBuildResult {
            result: BuildResult {
                build_log: storage.to_string(),
                rustc_version: self.rustc_version.clone(),
                docsrs_version: format!("docsrs {}", ::BUILD_VERSION),
                successful,
            },
            cargo_metadata,
            target: target.to_string(),
        })
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
        // cratesfyi/x86_64-pc-windows-msvc we still need target in this function.
        if !is_default_target {
            dest = dest.join(target);
        }

        info!("{} {}", source.display(), dest.display());
        copy_doc_dir(source, dest)
    }

    fn upload_docs(
        &self,
        conn: &Connection,
        name: &str,
        version: &str,
        local_storage: &Path,
    ) -> Result<()> {
        debug!("Adding documentation into database");
        add_path_into_database(
            conn,
            &format!("rustdoc/{}/{}", name, version),
            local_storage,
        )?;
        Ok(())
    }
}

struct FullBuildResult {
    result: BuildResult,
    target: String,
    cargo_metadata: CargoMetadata,
}

pub(crate) struct BuildResult {
    pub(crate) rustc_version: String,
    pub(crate) docsrs_version: String,
    pub(crate) build_log: String,
    pub(crate) successful: bool,
}
