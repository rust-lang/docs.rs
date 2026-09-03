use crate::{
    Config, docbuilder::build_error::RustwideBuildError, metrics::BuilderMetrics,
    utils::copy::copy_dir_all,
};
use anyhow::{Context as _, Error, Result};
use bytes::Bytes;
use docs_rs_build::{
    BUILDER_VERSION, BuildEnvironment, CpuLimit, ReleaseBuildResult, SandboxImageSource,
    TargetBuildResult,
};
use docs_rs_build_limits::{Limits, blacklist::is_blacklisted};
use docs_rs_build_queue::BuildPackageSummary;
use docs_rs_cargo_metadata::MetadataPackage;
use docs_rs_context::Context;
use docs_rs_database::{
    Pool,
    releases::{
        add_build_logs, add_doc_coverage, finish_build, finish_release, initialize_build,
        initialize_crate, initialize_release, update_build_with_error,
        update_crate_data_in_database,
    },
    service_config::{ConfigName, get_config, set_config},
};
use docs_rs_registry_api::RegistryApi;
use docs_rs_registry_api::ReleaseData;
use docs_rs_repository_stats::{RepositoryStatsUpdater, workspaces};
use docs_rs_rustdoc_json::{RUSTDOC_JSON_COMPRESSION_ALGORITHMS, RustdocJsonFormatVersion};
use docs_rs_storage::{
    AsyncStorage, Storage, compress, rustdoc_archive_path, rustdoc_json_path, source_archive_path,
};
use docs_rs_types::{
    BuildId, BuildStatus, CompressionAlgorithm, CrateId, KrateName, ReleaseId, Version,
};
use docs_rs_utils::{Handle, RUSTDOC_STATIC_STORAGE_PREFIX, retry, spawn_blocking};
use futures_util::future::try_join_all;
use regex::Regex;
use rustwide::{Crate, Toolchain};
use std::{
    collections::HashSet,
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    sync::Arc,
};
use tracing::{debug, error, info, info_span, instrument, warn};

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

#[derive(Debug)]
pub enum PackageKind<'a> {
    Local(&'a Path),
    CratesIo,
}

pub struct RustwideBuilder {
    environment: BuildEnvironment,
    runtime: Handle,
    config: Arc<Config>,
    db: Pool,
    blocking_storage: Arc<Storage>,
    storage: Arc<AsyncStorage>,
    registry_api: Arc<RegistryApi>,
    repository_stats: Arc<RepositoryStatsUpdater>,
    pub(crate) builder_metrics: Arc<BuilderMetrics>,
}

impl RustwideBuilder {
    pub fn init(config: Arc<Config>, context: &Context) -> Result<Self> {
        let runtime: Handle = context.runtime().clone().into();

        let toolchain = runtime.block_on(async {
            let mut conn = context.pool()?.get_async().await?;
            get_configured_toolchain(&mut conn).await
        })?;

        let default_limits = Limits::from_config(&config.build_limits);

        let cpu_limit = config
            .build_cpu_cores
            .as_ref()
            .map(|cores| CpuLimit::Cores(cores.0.clone()))
            .or_else(|| {
                config
                    .build_cpu_limit
                    .map(|limit| CpuLimit::Quota(limit as f32))
            });

        let sandbox_image = config
            .docker_image
            .as_ref()
            .map(|image| SandboxImageSource::LocalOrRemote(image.clone()))
            .unwrap_or_default();

        let environment = BuildEnvironment::builder(config.rustwide_workspace.as_path())
            .toolchain(toolchain)
            .running_inside_docker(config.inside_docker)
            .sandbox_image(sandbox_image)
            .fast_init(cfg!(test))
            .workspace_reinitialization_interval(config.build_workspace_reinitialization_interval)
            .maybe_cpu_limit(cpu_limit)
            .docker_runtime(config.docker_runtime)
            .include_default_targets(config.include_default_targets)
            .validate_host_resources(!config.disable_memory_limit)
            .maybe_compiler_metrics_collection_path(config.compiler_metrics_collection_path.clone())
            .default_limits(default_limits)
            .build()?;

        Ok(RustwideBuilder {
            environment,
            config: config.clone(),
            db: context.pool()?.clone(),
            runtime,
            blocking_storage: context.blocking_storage()?.clone(),
            storage: context.storage()?.clone(),
            registry_api: context.registry_api()?.clone(),
            repository_stats: context.repository_stats()?.clone(),
            builder_metrics: BuilderMetrics::new(context.meter_provider()).into(),
        })
    }

    /// Perform interval-based workspace and toolchain maintenance.
    ///
    /// This is the entry point used by the long-running build queue. When a
    /// compiler update is detected, its shared rustdoc files are published
    /// before another release is built.
    #[instrument(skip(self))]
    pub fn perform_maintenance(&mut self) -> Result<()> {
        self.sync_configured_toolchain()?;
        let maintenance = self.environment.perform_maintenance()?;
        self.publish_essential_files_if_needed(maintenance.toolchain_updated)
    }

    /// Force a toolchain update and publish new shared rustdoc files when needed.
    #[instrument(skip_all)]
    pub fn update_toolchain_and_add_essential_files(&mut self) -> Result<()> {
        self.sync_configured_toolchain()?;
        let updated = retry(|| self.environment.update_toolchain(), 3)
            .context("downloading new toolchain failed")?;

        debug!(updated, "toolchain update check complete");
        self.publish_essential_files_if_needed(updated)
    }

    fn sync_configured_toolchain(&mut self) -> Result<()> {
        let toolchain = self.runtime.block_on(async {
            let mut conn = self.db.get_async().await?;
            get_configured_toolchain(&mut conn).await
        })?;
        if self.environment.toolchain() != &toolchain {
            self.environment.set_toolchain(toolchain)?;
        }
        Ok(())
    }

    fn publish_essential_files_if_needed(&mut self, toolchain_updated: bool) -> Result<()> {
        let rustc_version = self.environment.rustc_version()?;
        let published_version = self.runtime.block_on(async {
            let mut conn = self.db.get_async().await?;
            get_config::<String>(&mut conn, ConfigName::RustcVersion).await
        })?;

        if toolchain_updated || published_version.as_ref() != Some(&rustc_version) {
            debug!(
                toolchain_updated,
                ?published_version,
                rustc_version,
                "publishing essential files"
            );
            self.add_essential_files()
                .context("adding essential files after toolchain maintenance")?;
        }
        Ok(())
    }

    // Retained for the existing integration tests while they transition to the
    // public lifecycle method above.
    #[cfg(test)]
    fn update_toolchain(&mut self) -> Result<bool> {
        self.environment.update_toolchain()
    }

    #[instrument(skip(self))]
    fn get_limits(&self, krate: &KrateName) -> Result<Limits> {
        self.runtime.block_on({
            let db = self.db.clone();
            let config = self.config.clone();
            async move {
                let mut conn = db.get_async().await?;
                Limits::for_crate(&config.build_limits, &mut conn, krate).await
            }
        })
    }

    pub fn add_essential_files(&mut self) -> Result<()> {
        let rustc_version = self.environment.rustc_version()?;
        info!("building a dummy crate to get essential files");
        let static_files = self.environment.build_essential_files()?.into_inner();
        self.runtime.block_on(
            self.storage
                .store_all(RUSTDOC_STATIC_STORAGE_PREFIX, &static_files),
        )?;
        self.runtime.block_on(async {
            let mut conn = self.db.get_async().await?;
            set_config(&mut conn, ConfigName::RustcVersion, rustc_version).await
        })?;
        Ok(())
    }

    pub fn build_local_package(&mut self, path: &Path) -> Result<BuildPackageSummary> {
        let metadata = self.environment.load_cargo_metadata(path).map_err(|err| {
            err.context(format!("failed to load local package {}", path.display()))
        })?;
        let package = metadata.root();
        self.build_package(
            &package
                .name
                .parse()
                .context("invalid crate name in package")?,
            &package.version,
            PackageKind::Local(path),
            false,
        )
    }

    #[instrument(skip(self))]
    pub fn build_package(
        &mut self,
        name: &KrateName,
        version: &Version,
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

                update_build_with_error(&mut conn, build_id, Some(&RustwideBuildError::Other(err)))
                    .await?;

                Ok(BuildPackageSummary {
                    successful: false,
                    should_reattempt: true,
                })
            }),
        }
    }

    #[instrument(skip(self))]
    #[allow(clippy::too_many_arguments)]
    fn build_package_inner(
        &mut self,
        name: &KrateName,
        version: &Version,
        kind: PackageKind<'_>,
        crate_id: CrateId,
        release_id: ReleaseId,
        build_id: BuildId,
        // Compiler metrics are now an environment-level setting and are
        // collected for every release when a destination is configured.
        _collect_metrics: bool,
    ) -> Result<bool> {
        info!("building package {} {}", name, version);

        let is_blacklisted = self.runtime.block_on(async {
            let mut conn = self.db.get_async().await?;

            let is_blacklisted = is_blacklisted(&mut conn, name).await?;

            Ok::<_, Error>(is_blacklisted)
        })?;

        if is_blacklisted {
            info!("skipping build of {}, crate has been blacklisted", name);
            return Ok(false);
        }

        let limits = self.get_limits(name)?;
        let is_local = matches!(kind, PackageKind::Local(_));
        let version_string = version.to_string();
        let krate = match kind {
            PackageKind::Local(path) => Crate::local(path),
            PackageKind::CratesIo => Crate::crates_io(name.as_str(), &version_string),
        };

        std::fs::create_dir_all(&self.config.temp_dir)?;
        let local_storage = tempfile::tempdir_in(&self.config.temp_dir)?;
        let source_dir = tempfile::tempdir_in(&self.config.temp_dir)?;

        let mut algs = HashSet::new();
        let fetched = self
            .environment
            .release(&krate)
            .limits(limits)
            .fetch()?
            .try_inspect(|fetched| fetched.copy_source_to(source_dir.path()))?;

        let source_stats = self.runtime.block_on(
            self.storage
                .store_all_in_archive(&source_archive_path(name, version), &source_dir),
        )?;
        algs.insert(source_stats.alg);

        let build = fetched.run(|build| build.build_docs())?;
        let memory_peak = build.statistics().memory_peak_bytes();
        let mut release = build.into_inner();
        let successful = release.successful();
        let has_docs = release.has_docs();
        let default_target = release.default_target().target.clone();

        let mut successful_targets = Vec::new();
        let documentation_size = if has_docs {
            for target in &release.targets {
                if target.successful() {
                    copy_target_docs(target, local_storage.path())?;
                    successful_targets.push(target.target.clone());
                }
            }

            let doc_stats = self.runtime.block_on(self.storage.store_all_in_archive(
                &rustdoc_archive_path(name, version),
                local_storage.path(),
            ))?;
            self.builder_metrics
                .documentation_size
                .record(doc_stats.original_size, &[]);
            algs.insert(doc_stats.alg);
            Some(doc_stats.original_size)
        } else {
            None
        };

        self.publish_json_and_build_logs(build_id, name, version, &mut release)?;

        let build_error = release
            .targets
            .first_mut()
            .and_then(|target| target.documentation.error.take());
        let rustc_version = self.environment.rustc_version()?;
        let docsrs_version = format!("docsrs {BUILDER_VERSION}");
        let mut async_conn = self.runtime.block_on(self.db.get_async())?;
        self.runtime.block_on(finish_build(
            &mut async_conn,
            build_id,
            &rustc_version,
            &docsrs_version,
            if successful {
                BuildStatus::Success
            } else {
                BuildStatus::Failure
            },
            documentation_size,
            memory_peak,
            build_error.as_ref(),
        ))?;

        if successful {
            self.builder_metrics.successful_builds.add(1, &[]);
        } else if release.cargo_metadata.root().is_library() {
            self.builder_metrics.failed_builds.add(1, &[]);
        } else {
            self.builder_metrics.non_library_builds.add(1, &[]);
        }

        let release_data = if !is_local {
            match self
                .runtime
                .block_on(self.registry_api.get_release_data(name, version))
            {
                Ok(data) => data,
                Err(err) => {
                    error!(%name, %version, ?err, "could not fetch releases-data");
                    None
                }
            }
        } else {
            None
        }
        .unwrap_or_else(ReleaseData::dummy);

        let cargo_metadata = release.cargo_metadata.root();
        let repository = self.get_repo(cargo_metadata)?;
        let current_release_build_status = self.runtime.block_on(
            sqlx::query_scalar!(
                r#"
                    SELECT build_status AS "build_status: BuildStatus"
                    FROM release_build_status
                    WHERE rid = $1
                    "#,
                release_id.0,
            )
            .fetch_optional(&mut *async_conn),
        )?;

        if !successful && current_release_build_status == Some(BuildStatus::Success) {
            info!(
                "build was unsuccessful, but the release was already successfully built in the past. Skipping release record update."
            );
            return Ok(false);
        }

        let has_examples = source_dir.path().join("examples").is_dir();
        self.runtime.block_on(finish_release(
            &mut async_conn,
            crate_id,
            release_id,
            cargo_metadata,
            source_dir.path(),
            &default_target,
            successful_targets,
            &release_data,
            has_docs,
            has_examples,
            algs,
            repository,
            source_stats.original_size,
        ))?;

        if let Some(repository_id) = repository {
            self.runtime.block_on(workspaces::update_repository_stats(
                &mut async_conn,
                repository_id,
            ))?;
        }

        if let Some(doc_coverage) = release
            .targets
            .first_mut()
            .and_then(|target| target.coverage.output.take())
            .flatten()
        {
            self.runtime
                .block_on(add_doc_coverage(&mut async_conn, release_id, doc_coverage))?;
        }

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

        if successful {
            for prefix in &["rustdoc", "sources"] {
                let prefix = format!("{prefix}/{name}/{version}/");
                debug!("cleaning old storage folder {}", prefix);
                self.blocking_storage.delete_prefix(&prefix)?;
            }
        }

        self.runtime.block_on(async move {
            drop(async_conn);
        });
        local_storage.close()?;
        Ok(successful)
    }

    #[instrument(skip(self, release))]
    fn publish_json_and_build_logs(
        &self,
        build_id: BuildId,
        name: &KrateName,
        version: &Version,
        release: &mut ReleaseBuildResult,
    ) -> Result<()> {
        let mut build_logs = Vec::new();

        for target in &mut release.targets {
            let json_log_path = format!("build-logs/{build_id}/{}_json.txt", target.target);
            self.blocking_storage
                .store_one(json_log_path, std::mem::take(&mut target.rustdoc_json.log))?;

            if let Some(json) = &target.rustdoc_json.output {
                let upload = json.format_version().and_then(|format_version| {
                    self.runtime.block_on(try_join_all(
                        RUSTDOC_JSON_COMPRESSION_ALGORITHMS.iter().map(|algorithm| {
                            self.upload_json_output(
                                name,
                                version,
                                &target.target,
                                format_version,
                                *algorithm,
                                json.path().to_owned(),
                            )
                        }),
                    ))?;
                    Ok(())
                });
                if let Err(error) = upload {
                    error!(
                        ?error,
                        target = target.target,
                        "internal error while publishing rustdoc JSON output"
                    );
                }
            }

            let successful = target.successful();
            let log_name = format!("{}.txt", target.target);
            self.blocking_storage.store_one(
                format!("build-logs/{build_id}/{log_name}"),
                std::mem::take(&mut target.documentation.log),
            )?;
            build_logs.push((log_name, successful));
        }

        let mut conn = self.runtime.block_on(self.db.get_async())?;
        self.runtime
            .block_on(add_build_logs(&mut conn, build_id, build_logs))?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn upload_json_output(
        &self,
        name: &KrateName,
        version: &Version,
        target: &str,
        format_version: RustdocJsonFormatVersion,
        alg: CompressionAlgorithm,
        json_filename: PathBuf,
    ) -> Result<()> {
        let json_file_size = json_filename.metadata()?.len();
        let compressed_json = spawn_blocking(move || {
            let compress_span = info_span!(
                "compress_json",
                file_size = json_file_size,
                algorithm = %alg
            );
            let _span = compress_span.enter();

            let compressed = compress(BufReader::new(File::open(&json_filename)?), alg)?;
            Ok(Bytes::from(compressed))
        })
        .await?;

        try_join_all(
            [
                rustdoc_json_path(name, version, target, format_version, Some(alg)),
                rustdoc_json_path(
                    name,
                    version,
                    target,
                    RustdocJsonFormatVersion::Latest,
                    Some(alg),
                ),
            ]
            .map(|path| {
                let compressed_json = compressed_json.clone();
                async move {
                    self.storage
                        .store_one_uncompressed(&path, compressed_json)
                        .await
                }
            }),
        )
        .await?;

        Ok(())
    }

    fn get_repo(&self, metadata: &MetadataPackage) -> Result<Option<i32>> {
        self.runtime
            .block_on(self.repository_stats.load_repository(metadata))
    }
}

#[instrument(skip(result))]
fn copy_target_docs(result: &TargetBuildResult, destination: &Path) -> Result<()> {
    let source = result
        .documentation
        .output
        .as_ref()
        .context("successful documentation build has no output directory")?;
    let destination = if result.is_default {
        destination.to_owned()
    } else {
        destination.join(&result.target)
    };

    info!(
        source = %source.display(),
        destination = %destination.display(),
        "copying documentation"
    );
    Ok(copy_dir_all(source, destination)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{TestEnvironment, TestEnvironmentExt as _};
    use docs_rs_registry_api::ReleaseData;
    use docs_rs_types::{
        BuildStatus, CompressionAlgorithm, ReleaseId, SimpleBuildError, Version, testing::V0_1,
    };
    use docs_rs_utils::block_on_async_with_conn;
    use docsrs_metadata::DEFAULT_TARGETS;
    use pretty_assertions::assert_eq;
    use std::{collections::BTreeMap, iter, sync::LazyLock};

    static DUMMY_CRATE_NAME: LazyLock<KrateName> =
        LazyLock::new(|| "empty-library".parse().unwrap());
    const DUMMY_CRATE_VERSION: Version = Version::new(1, 0, 0);

    #[test]
    #[ignore]
    fn test_build_crate() -> Result<()> {
        let env = TestEnvironment::new()?;

        let crate_ = &*DUMMY_CRATE_NAME;
        let crate_path = crate_.as_str().replace('-', "_");
        let version = DUMMY_CRATE_VERSION;
        let default_target = "x86_64-unknown-linux-gnu";

        let storage = env.blocking_storage()?;
        let old_rustdoc_file = format!("rustdoc/{crate_}/{version}/some_doc_file");
        let old_source_file = format!("sources/{crate_}/{version}/some_source_file");
        storage.store_one(&old_rustdoc_file, Vec::new())?;
        storage.store_one(&old_source_file, Vec::new())?;

        let mut builder = env.build_builder()?;
        builder.update_toolchain()?;
        assert!(
            builder
                .build_package(crate_, &version, PackageKind::CratesIo, false)?
                .successful
        );

        // check release record in the db (default and other targets)
        let row = block_on_async_with_conn!(env, |mut conn| async {
            sqlx::query!(
                r#"SELECT
                        r.rustdoc_status,
                        r.default_target,
                        r.doc_targets,
                        r.source_size as "source_size!",
                        cov.total_items,
                        b.id as build_id,
                        b.build_status::TEXT as build_status,
                        b.docsrs_version,
                        b.rustc_version,
                        b.documentation_size,
                        b.memory_peak,
                        (
                            SELECT array_agg(row(bl.log_filename, bl.success))
                            FROM (
                                SELECT log_filename, success
                                FROM builds_logs
                                WHERE builds_logs.build_id = b.id
                                ORDER BY id
                            ) bl
                        ) AS "logs: Vec<(String, bool)>"
                    FROM
                        crates as c
                        INNER JOIN releases AS r ON c.id = r.crate_id
                        INNER JOIN builds as b ON r.id = b.rid
                        LEFT OUTER JOIN doc_coverage AS cov ON r.id = cov.release_id
                    WHERE
                        c.name = $1 AND
                        r.version = $2"#,
                crate_ as _,
                version as _,
            )
            .fetch_one(&mut *conn)
            .await
            .map_err(Into::into)
        })?;

        assert_eq!(row.rustdoc_status, Some(true));
        assert_eq!(row.default_target, Some(default_target.into()));
        assert!(row.total_items.is_some());
        assert!(!row.docsrs_version.unwrap().is_empty());
        assert!(!row.rustc_version.unwrap().is_empty());
        assert_eq!(row.build_status.unwrap(), "success");
        assert!(row.source_size > 0);
        assert!(row.documentation_size.unwrap() > 0);
        assert!(row.memory_peak.unwrap() > 10 * 1024 * 1024); // 10 MiB, in my test it was > 100 MiB
        let mut logs = row.logs.unwrap();
        logs.sort();
        let mut expected = vec![
            ("x86_64-unknown-linux-gnu.txt".to_owned(), true),
            ("i686-pc-windows-msvc.txt".to_owned(), true),
            ("aarch64-unknown-linux-gnu.txt".to_owned(), true),
            ("x86_64-pc-windows-msvc.txt".to_owned(), true),
            ("aarch64-apple-darwin.txt".to_owned(), true),
        ];
        expected.sort();

        assert_eq!(logs, expected);

        let mut targets: Vec<String> = row
            .doc_targets
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        targets.sort();

        let _runtime = env.runtime();
        // FIXME: do we want to keep these tests? how to emulate it?
        // or move this to "integration tests" on the crate level?
        // let web = runtime.block_on(env.web_app());

        // old rustdoc & source files are gone
        assert!(!storage.exists(&old_rustdoc_file)?);
        assert!(!storage.exists(&old_source_file)?);

        // doc archive exists
        let doc_archive = rustdoc_archive_path(crate_, &version);
        assert!(storage.exists(&doc_archive)?, "{}", doc_archive);

        // source archive exists
        let source_archive = source_archive_path(crate_, &version);
        assert!(storage.exists(&source_archive)?, "{}", source_archive);

        // default target was built and is accessible
        assert!(storage.exists_in_archive(
            &doc_archive,
            None,
            &format!("{crate_path}/index.html"),
        )?);
        // runtime.block_on(web.assert_success(&format!("/{crate_}/{version}/{crate_path}/")))?;

        // source is also packaged
        assert!(storage.exists_in_archive(&source_archive, None, "src/lib.rs",)?);
        // runtime.block_on(
        //     web.assert_success(&format!("/crate/{crate_}/{version}/source/src/lib.rs")),
        // )?;
        assert!(!storage.exists_in_archive(
            &doc_archive,
            None,
            &format!("{default_target}/{crate_path}/index.html"),
        )?);

        let _default_target_url = format!("/{crate_}/{version}/{default_target}/{crate_path}/");
        // runtime.block_on(web.assert_redirect(
        //     &default_target_url,
        //     &format!("/{crate_}/{version}/{crate_path}/"),
        // ))?;

        // Non-dist toolchains only have a single target, and of course
        // if include_default_targets is false we won't have this full list
        // of targets.
        if builder.environment.toolchain().as_dist().is_some()
            && env.config().include_default_targets
        {
            assert_eq!(
                targets,
                vec![
                    "aarch64-apple-darwin",
                    "aarch64-unknown-linux-gnu",
                    "i686-pc-windows-msvc",
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
                        &version,
                        target,
                        RustdocJsonFormatVersion::Latest,
                        Some(*alg),
                    );
                    assert!(storage.exists(&path)?);

                    let ext = alg.file_extension();

                    let json_prefix = format!("rustdoc-json/{crate_}/{version}/{target}/");
                    let mut json_files: Vec<_> = storage
                        .list_prefix(&json_prefix)
                        .filter_map(|res| res.ok())
                        .map(|f| f.strip_prefix(&json_prefix).unwrap().to_owned())
                        .collect();
                    json_files.retain(|f| f.ends_with(&format!(".json.{ext}")));
                    json_files.sort();
                    assert!(json_files[0].starts_with(&format!("empty-library_1.0.0_{target}_")));
                    assert!(json_files[0].ends_with(&format!(".json.{ext}")));
                    assert_eq!(
                        json_files[1],
                        format!("empty-library_1.0.0_{target}_latest.json.{ext}")
                    );
                }

                if *target == default_target {
                    continue;
                }
                let target_docs_present = storage.exists_in_archive(
                    &doc_archive,
                    None,
                    &format!("{target}/{crate_path}/index.html"),
                )?;

                let _target_url = format!("/{crate_}/{version}/{target}/{crate_path}/index.html");

                assert!(target_docs_present);
                // runtime.block_on(web.assert_success(&target_url))?;

                assert!(
                    storage
                        .exists(&format!("build-logs/{}/{target}.txt", row.build_id))
                        .unwrap()
                );
            }
        }
        Ok(())
    }

    #[test]
    #[ignore]
    fn test_build_binary_crate() -> Result<()> {
        let env = TestEnvironment::new()?;

        // some binary crate
        let crate_ = KrateName::from_static("heater");
        let version = Version::new(0, 2, 3);

        let storage = env.blocking_storage()?;
        let old_rustdoc_file = format!("rustdoc/{crate_}/{version}/some_doc_file");
        let old_source_file = format!("sources/{crate_}/{version}/some_source_file");
        storage.store_one(&old_rustdoc_file, Vec::new())?;
        storage.store_one(&old_source_file, Vec::new())?;

        let mut builder = env.build_builder()?;
        builder.update_toolchain()?;
        assert!(
            !builder
                .build_package(&crate_, &version, PackageKind::CratesIo, false)?
                .successful
        );

        // check release record in the db (default and other targets)
        let row = block_on_async_with_conn!(env, |mut conn| async {
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
                crate_ as _,
                version as _
            )
            .fetch_one(&mut *conn)
            .await
            .map_err(Into::into)
        })?;

        assert_eq!(row.rustdoc_status, Some(false));
        assert_eq!(row.is_library, Some(false));

        // doc archive exists
        let doc_archive = rustdoc_archive_path(&crate_, &version);
        assert!(!storage.exists(&doc_archive)?);

        // source archive exists
        let source_archive = source_archive_path(&crate_, &version);
        assert!(storage.exists(&source_archive)?);

        // old rustdoc & source files still exist
        assert!(storage.exists(&old_rustdoc_file)?);
        assert!(storage.exists(&old_source_file)?);

        Ok(())
    }

    #[test]
    #[ignore]
    fn test_failed_build_with_existing_successful_release() -> Result<()> {
        let env = TestEnvironment::new()?;

        // rand 0.8.5 fails to build with recent nightly versions
        // https://github.com/rust-lang/docs.rs/issues/26750
        let crate_ = KrateName::from_static("rand");
        let version = Version::new(0, 8, 5);

        // create a successful release & build in the database
        let release_id = block_on_async_with_conn!(env, |mut conn| async {
            let crate_id = initialize_crate(&mut *conn, &crate_).await?;
            let release_id = initialize_release(&mut *conn, crate_id, &version).await?;
            let build_id = initialize_build(&mut *conn, release_id).await?;
            finish_build(
                &mut *conn,
                build_id,
                "some-version",
                "other-version",
                BuildStatus::Success,
                None,
                None,
                None::<&SimpleBuildError>,
            )
            .await?;
            finish_release(
                &mut *conn,
                crate_id,
                release_id,
                &MetadataPackage {
                    name: crate_.as_str().into(),
                    version: version.clone(),
                    id: "".into(),
                    license: None,
                    repository: None,
                    homepage: None,
                    description: None,
                    documentation: None,
                    dependencies: vec![],
                    targets: vec![],
                    readme: None,
                    keywords: vec![],
                    features: BTreeMap::new(),
                },
                Path::new("/unknown/"),
                "x86_64-unknown-linux-gnu",
                vec![
                    "i686-pc-windows-msvc".into(),
                    "aarch64-unknown-linux-gnu".into(),
                    "aarch64-apple-darwin".into(),
                    "x86_64-pc-windows-msvc".into(),
                    "x86_64-unknown-linux-gnu".into(),
                ],
                &ReleaseData::dummy(),
                true,
                false,
                iter::once(CompressionAlgorithm::Deflate),
                None,
                42,
            )
            .await?;

            Ok(release_id)
        })?;

        fn check_rustdoc_status(env: &TestEnvironment, rid: ReleaseId) -> Result<()> {
            assert_eq!(
                block_on_async_with_conn!(env, |mut conn| async {
                    sqlx::query_scalar!("SELECT rustdoc_status FROM releases WHERE id = $1", rid.0)
                        .fetch_one(&mut *conn)
                        .await
                        .map_err(Into::into)
                })?,
                Some(true)
            );
            Ok(())
        }

        check_rustdoc_status(&env, release_id)?;

        let mut builder = env.build_builder()?;
        builder.update_toolchain()?;
        assert!(
            // not successful build
            !builder
                .build_package(&crate_, &version, PackageKind::CratesIo, false)?
                .successful
        );

        check_rustdoc_status(&env, release_id)?;
        Ok(())
    }

    #[test]
    #[ignore]
    fn test_sources_are_added_even_for_build_failures_before_build() -> Result<()> {
        let env = TestEnvironment::new()?;

        // https://github.com/rust-lang/docs.rs/issues/2523
        // package with invalid cargo metadata.
        // Will succeed in the crate fetch step, so sources are
        // added. Will fail when we try to build.
        let crate_ = KrateName::from_static("simple-build-failure");
        let version = V0_1;
        let test_crate = Path::new("../../lib/docs_rs_build/tests/fixtures/simple-build-failure/");

        let mut builder = env.build_builder()?;
        builder.update_toolchain()?;

        // `Result` is `Ok`, but the build-result is `false`
        assert!(!builder.build_local_package(test_crate)?.successful);

        // source archive exists
        let source_archive = source_archive_path(&crate_, &version);
        let storage = env.blocking_storage()?;

        assert!(
            storage.exists(&source_archive)?,
            "archive doesnt exist: {source_archive}"
        );
        assert!(
            storage
                .fetch_source_file(&crate_, &version, None, "src/main.rs")
                .is_ok()
        );

        Ok(())
    }

    #[test]
    #[ignore]
    fn test_build_failures_before_build() -> Result<()> {
        let env = TestEnvironment::new()?;

        // https://github.com/rust-lang/docs.rs/issues/2491
        // package without Cargo.toml, so fails directly in the fetch stage.
        let crate_ = KrateName::from_static("emheap");
        let version = Version::new(0, 1, 0);
        let mut builder = env.build_builder()?;
        builder.update_toolchain()?;

        // `Result` is `Ok`, but the build-result is `false`
        let summary = builder.build_package(&crate_, &version, PackageKind::CratesIo, false)?;

        assert!(!summary.successful);
        assert!(summary.should_reattempt);

        let row = block_on_async_with_conn!(env, |mut conn| async {
            sqlx::query!(
                r#"SELECT
                   rustc_version,
                   docsrs_version,
                   build_status as "build_status: BuildStatus",
                   error_kind,
                   errors
                   FROM
                   crates as c
                   INNER JOIN releases as r on c.id = r.crate_id
                   INNER JOIN builds as b on b.rid = r.id
                   WHERE c.name = $1 and r.version = $2"#,
                crate_ as _,
                version as _,
            )
            .fetch_one(&mut *conn)
            .await
            .map_err(Into::into)
        })?;

        assert!(row.rustc_version.is_none());
        assert!(row.docsrs_version.is_none());
        assert_eq!(row.build_status, BuildStatus::Failure);
        assert_eq!(row.error_kind, Some("Other".into()));
        assert!(row.errors.unwrap().contains("missing Cargo.toml"));

        Ok(())
    }
}
