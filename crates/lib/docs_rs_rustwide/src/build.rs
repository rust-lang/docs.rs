use crate::{
    BuildEnvironment, BuildStepError, HtmlOutput, ReleaseBuildResult, RustdocJsonOutput,
    StepFailure, StepReport, StepResult, TargetBuildResult, command::PrepareCommand,
    utils::copy_dir_all,
};
use anyhow::{Context as _, Result, anyhow, bail};
use bon::bon;
use bytesize::ByteSize;
use docs_rs_build_limits::Limits;
use docs_rs_cargo_metadata::CargoMetadata;
use docs_rs_types::doc_coverage::{self, DocCoverage};
use docsrs_metadata::{BuildTargets, HOST_TARGET, Metadata};
use rustwide::{
    Build,
    cmd::Command,
    logging::{self, LogStorage},
};
use std::{
    cell::RefCell,
    collections::HashSet,
    ffi::OsStr,
    fmt,
    fs::{self, File},
    io::{BufRead as _, BufReader},
    path::{Path, PathBuf},
    time::Instant,
};
use tracing::{Span, debug, error, info, instrument};

/// Name of rustdoc's documentation output directory.
const DOC_OUTPUT_DIR_NAME: &str = "doc";

#[cfg(test)]
mod policy_tests;

#[derive(Debug)]
pub enum Emit {
    HtmlStaticFiles,
    HtmlNonStaticFiles,
}

impl Emit {
    pub fn as_str(&self) -> &str {
        match self {
            Self::HtmlStaticFiles => "html-static-files",
            Self::HtmlNonStaticFiles => "html-non-static-files",
        }
    }
}

impl fmt::Display for Emit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn capture_step<T>(run: impl FnOnce() -> Result<T, BuildStepError>) -> StepResult<T> {
    let started = Instant::now();
    let outcome = run();
    let duration = started.elapsed();

    match outcome {
        Ok(value) => Ok(StepReport {
            value,
            duration: duration.into(),
            log: None,
        }),
        Err(error) => Err(StepReport {
            value: error,
            duration: duration.into(),
            log: None,
        }),
    }
}

fn capture_rustwide_step<T>(
    max_log_size: ByteSize,
    run: impl FnOnce() -> Result<T, BuildStepError>,
) -> StepResult<T> {
    let mut storage = LogStorage::new(log::LevelFilter::Info);
    storage.set_max_size(max_log_size.as_u64() as usize);

    let mut result = capture_step(|| logging::capture(&storage, run));

    let log = Some(storage.to_string());
    match result {
        Ok(ref mut r) => r.log = log,
        Err(ref mut r) => r.log = log,
    }

    result
}

/// Load Cargo metadata for a source tree with the configured toolchain.
#[instrument(skip_all)]
pub fn load_cargo_metadata<'build, 'ws>(
    environment: &'build BuildEnvironment,
    build: &'build Build<'ws>,
    limits: &'build Limits,
) -> StepResult<CargoMetadata> {
    capture_rustwide_step(limits.max_log_size(), || {
        read_cargo_metadata(environment, build)
    })
}

fn read_cargo_metadata(
    environment: &BuildEnvironment,
    build: &Build<'_>,
) -> Result<CargoMetadata, BuildStepError> {
    let source_dir = &build.host_source_dir();

    debug!(source_dir=%source_dir.display(), "loading Cargo metadata");
    let output = Command::new(
        environment.workspace(),
        environment.configured_toolchain().cargo(),
    )
    .args(["metadata", "--format-version", "1"])
    .current_directory(source_dir)
    .log_output(false)
    .run_capture()
    .map_err(BuildStepError::Command)?;

    BuildStepError::as_output(|| {
        let [metadata] = output.stdout_lines() else {
            bail!("invalid output returned by `cargo metadata`");
        };

        let metadata = CargoMetadata::load_from_metadata(metadata)?;
        debug!("Cargo metadata loaded");
        Ok(metadata)
    })
}

/// A prepared release inside an active rustwide sandbox.
pub struct ReleaseBuild<'build, 'ws> {
    pub(crate) environment: &'build BuildEnvironment,
    pub(crate) build: &'build Build<'ws>,
    pub(crate) docsrs_metadata: Metadata,
    pub(crate) cargo_metadata: RefCell<CargoMetadata>,
    pub(crate) limits: &'build Limits,
    pub(crate) resource_suffix: String,
    fetched_build_std_targets: RefCell<HashSet<String>>,
}

#[bon]
impl<'build, 'ws> ReleaseBuild<'build, 'ws> {
    #[instrument(skip_all)]
    pub(crate) fn new(
        environment: &'build BuildEnvironment,
        build: &'build Build<'ws>,
        limits: &'build Limits,
    ) -> Result<Self> {
        debug!("reading docs.rs metadata");
        let docsrs_metadata = Metadata::from_crate_root(build.host_source_dir())?;
        debug!("reading cargo metadata");
        let cargo_metadata = load_cargo_metadata(environment, build, limits)
            .context("error loading cargo metadata")?
            .into_inner();

        let resource_suffix = environment.resource_suffix()?;
        debug!(resource_suffix, "release build prepared");

        Ok(Self {
            environment,
            build,
            cargo_metadata: RefCell::new(cargo_metadata),
            docsrs_metadata,
            limits,
            resource_suffix,
            fetched_build_std_targets: RefCell::new(HashSet::new()),
        })
    }

    pub(crate) fn build_rustwide_command<'pl>(&self) -> Command<'ws, 'pl> {
        let mut command = self
            .build
            .cargo()
            .timeout(Some(self.limits.timeout().into()))
            .no_output_timeout(None);

        for (key, value) in self.docsrs_metadata.environment_variables() {
            command = command.env(key, value);
        }
        command
    }

    /// Prepare the Cargo command used by docs.rs for one documentation target.
    ///
    /// The command runs inside this build's sandbox. Dependencies must be
    /// fetched beforehand because docs.rs invokes Cargo in offline mode.
    pub fn command<'release_build>(
        &'release_build self,
        target: impl Into<String>,
    ) -> PrepareCommand<'release_build, 'build, 'ws> {
        PrepareCommand::new(self, target)
    }

    pub(crate) fn temp_dir(&self) -> Result<PathBuf> {
        // first find the "build" dir rustwide manages, and doesn't expose yet.
        // It's the shared parent of `host_target_dir` and `host_source_dir`.
        //
        // We don't use the system-wide tmp, so we're sure the tmp dir is on the same filesystem.
        //
        // We might add this to rustwide.

        let tmp_dir = {
            let host_target_dir = self.build.host_target_dir();
            let parent = host_target_dir.parent().expect("always has a parent");
            parent.join("tmp")
        };

        fs::create_dir_all(&tmp_dir)?;

        Ok(tmp_dir)
    }

    /// Return the host path containing documentation for a target.
    ///
    /// Cargo places proc-macro documentation in the host target directory even
    /// when a target argument is otherwise in use.
    pub(crate) fn output_dir(&self, target: &str) -> PathBuf {
        if self.docsrs_metadata.proc_macro {
            self.build.host_target_dir().join(DOC_OUTPUT_DIR_NAME)
        } else {
            self.build
                .host_target_dir()
                .join(target)
                .join(DOC_OUTPUT_DIR_NAME)
        }
    }

    /// Targets selected by this release's docs.rs metadata.
    /// Fall back to the default target list, or the host-target.
    pub fn metadata_targets(&self) -> BuildTargets<'_> {
        self.docsrs_metadata
            .targets(self.environment.includes_default_targets())
    }

    /// Fetch dependencies needed by `-Zbuild-std` before offline commands run.
    #[instrument(skip_all)]
    pub(crate) fn fetch_build_std_dependencies<'a>(
        &self,
        targets: impl IntoIterator<Item = &'a str>,
    ) -> Result<()> {
        let missing_targets: Vec<_> = {
            let fetched_targets = self.fetched_build_std_targets.borrow();
            targets
                .into_iter()
                .filter(|target| !fetched_targets.contains(*target))
                .collect()
        };

        if missing_targets.is_empty() {
            debug!("build-std dependencies are already fetched");
            return Ok(());
        }

        debug!(?missing_targets, "fetching build-std dependencies");
        self.build.fetch_build_std_dependencies(&missing_targets)?;
        self.fetched_build_std_targets
            .borrow_mut()
            .extend(missing_targets.into_iter().map(str::to_owned));
        debug!("build-std dependencies fetched");
        Ok(())
    }

    /// Metadata parsed from the prepared crate source.
    pub fn docsrs_metadata(&self) -> &Metadata {
        &self.docsrs_metadata
    }

    /// Limits applied to this release.
    pub fn limits(&self) -> &Limits {
        self.limits
    }

    /// Build coverage, rustdoc JSON, and HTML for the full docs.rs target set.
    ///
    /// All commands execute through the same rustwide build and reusable
    /// sandbox. All step failures, including lockfile regeneration failures,
    /// remain in target results with their duration and logs.
    /// Additional targets are built only when the default target produces
    /// library documentation.
    #[instrument(skip_all, fields(crate_name, crate_version))]
    pub fn build_docs(&self) -> ReleaseBuildResult {
        let metadata_targets = self.metadata_targets();
        let default_target = metadata_targets.default_target;
        let other_targets: Vec<_> = metadata_targets
            .other_targets
            .into_iter()
            .take(self.limits.targets())
            .collect();

        debug!(
            default_target,
            ?other_targets,
            "selected documentation targets"
        );

        {
            let metadata = self.cargo_metadata.borrow();
            let root_package = metadata.root();
            Span::current()
                .record("crate_name", root_package.name.as_str())
                .record(
                    "crate_version",
                    tracing::field::display(&root_package.version),
                );
        }

        let default_target_build = self
            .build_target(default_target)
            .retry_without_lockfile(true)
            .build_coverage(true)
            .run();

        let default_has_docs = self
            .cargo_metadata
            .borrow()
            .root()
            .library_name()
            .is_some_and(|name| default_target_build.has_docs(&name));

        let mut target_results = vec![];

        if default_has_docs {
            for target in other_targets {
                target_results.push(self.build_target(target).run());
            }
        } else {
            info!("default target produced no library documentation; skipping other targets");
        }

        ReleaseBuildResult {
            statistics: self.build.statistics(),
            docsrs_metadata: self.docsrs_metadata.clone(),
            cargo_metadata: self.cargo_metadata.borrow().clone(),
            default_target: default_target_build,
            other_targets: target_results,
        }
    }

    /// Build coverage, rustdoc JSON, and HTML for one target.
    ///
    /// Coverage and JSON failures are retained and HTML is still attempted.
    /// When requested, an HTML command failure retries all steps once with a
    /// regenerated lockfile if one exists. If regeneration fails, no retry runs;
    /// the original results and regeneration failure are retained together.
    /// Metrics collection is a separate, nonfatal step after each HTML attempt.
    #[builder(finish_fn(name=run))]
    pub fn build_target(
        &self,
        #[builder(start_fn)] target: &str,
        #[builder(default = false)] retry_without_lockfile: bool,
        #[builder(default = false)] build_coverage: bool,
    ) -> TargetBuildResult {
        let started = Instant::now();

        let mut target_result = self.build_target_once(target, build_coverage);

        if retry_without_lockfile
            && matches!(
                &target_result.documentation,
                Err(StepReport {
                    value: BuildStepError::Command(_),
                    ..
                })
            )
            && self.build.host_source_dir().join("Cargo.lock").exists()
        {
            debug!(
                target,
                "target build failed; retrying with a regenerated lockfile"
            );

            let regeneration = self.regenerate_lockfile();
            if regeneration.is_ok() {
                target_result = self.build_target_once(target, build_coverage);
            }
            target_result.regenerate_lockfile = Some(regeneration);
        }

        target_result.duration = Some(started.elapsed().into());
        target_result
    }

    #[instrument(skip_all, fields(target, build_coverage))]
    fn build_target_once(&self, target: &str, build_coverage: bool) -> TargetBuildResult {
        // Coverage must precede the HTML build because Cargo currently clears
        // rustdoc's target output directory between these invocations.
        let coverage = if build_coverage {
            self.build_coverage(target)
        } else {
            capture_step(|| Ok(None))
        };

        let rustdoc_json = self.build_rustdoc_json(target);
        let documentation = self.build_documentation(target);

        let compiler_metrics = self
            .collect_compiler_metrics()
            .inspect_err(|err| error!(?err, "error collecting compiler metrics after target build"))
            .ok()
            .flatten();

        let is_default = target == self.metadata_targets().default_target;

        if documentation.is_ok() && self.docsrs_metadata().proc_macro {
            assert!(
                is_default && target == HOST_TARGET,
                "can't handle cross-compiling macros"
            );
        }

        TargetBuildResult {
            duration: None,
            target: target.into(),
            is_default,
            documentation,
            rustdoc_json,
            compiler_metrics,
            coverage,
            regenerate_lockfile: None,
        }
    }

    /// Collect documentation coverage for one target.
    ///
    /// All failures retain their duration and log; the caller decides whether to abort.
    #[instrument(skip_all, fields(target))]
    pub fn build_coverage(&self, target: &str) -> StepResult<Option<DocCoverage>> {
        self.capture_rustwide_step(|| {
            self.command(target)
                .rustdoc_args(["--output-format", "json", "--show-coverage"])
                .prepare()
                .map_err(BuildStepError::Prepare)?
                .log_output(true)
                .run()
                .map_err(BuildStepError::Command)?;

            BuildStepError::as_output(|| {
                let output_dir = self.output_dir(target);
                let path = find_single_output_file(&output_dir, "json")?;
                let reader = BufReader::new(File::open(path)?);

                let mut coverage = DocCoverage::default();
                for line in reader.lines() {
                    let line = line?;
                    coverage.extend(
                        doc_coverage::parse_line(&line).context("parsing coverage output")?,
                    );
                }

                Ok((!coverage.is_empty()).then_some(coverage))
            })
        })
    }

    /// Build unstable rustdoc JSON for one target.
    ///
    /// All failures retain their duration and log; the caller decides whether to abort.
    #[instrument(skip_all, fields(target))]
    pub fn build_rustdoc_json(&self, target: &str) -> StepResult<RustdocJsonOutput> {
        self.capture_rustwide_step(|| {
            self.command(target)
                .rustdoc_args(["--output-format", "json"])
                .prepare()
                .map_err(BuildStepError::Prepare)?
                .run()
                .map_err(BuildStepError::Command)?;

            BuildStepError::as_output(|| {
                let output_file = find_single_output_file(self.output_dir(target), "json")?;

                let destination = tempfile::Builder::new()
                    .prefix(&format!(
                        "{}.",
                        output_file.file_stem().unwrap().to_string_lossy()
                    ))
                    .suffix(&format!(
                        ".{}",
                        output_file.extension().unwrap().to_string_lossy()
                    ))
                    .tempfile_in(&self.temp_dir()?)?
                    .into_temp_path();

                fs::rename(&output_file, &destination)
                    .context("couldn't move output file to temp destination")?;

                Ok(RustdocJsonOutput::new(destination.keep()?))
            })
        })
    }

    /// Build HTML documentation without emitting shared static files.
    ///
    /// All failures retain their duration and log; the caller decides whether to abort.
    pub fn build_documentation(&self, target: &str) -> StepResult<HtmlOutput> {
        self.build_html(target, Emit::HtmlNonStaticFiles)
    }

    #[instrument(skip_all)]
    pub(crate) fn build_essential_files(&self) -> StepResult<HtmlOutput> {
        let mut result = self.build_html(docsrs_metadata::HOST_TARGET, Emit::HtmlStaticFiles)?;

        // we keep the original duration & log from the build-html step,
        // changing / testing the output dir doesn't change much here.

        let static_files = result.value.path().join("static.files");
        if !static_files.is_dir() {
            return Err(StepFailure {
                duration: result.duration,
                log: result.log,
                value: BuildStepError::Output(anyhow!(
                    "essential-files build did not produce {}",
                    static_files.display()
                )),
            });
        } else {
            // just change the path from the documentation root to the static.files
            // subdirectory. We're still inside the same tempdir.
            result.value.path = static_files;
        }

        Ok(result)
    }

    #[instrument(skip_all, fields(target, emit))]
    fn build_html(&self, target: &str, emit: Emit) -> StepResult<HtmlOutput> {
        self.capture_rustwide_step(|| {
            let mut command = self
                .command(target)
                .rustdoc_arg(format!("--emit={emit}"))
                .rustdoc_args(["--resource-suffix", &self.resource_suffix])
                .cargo_arg("-Zrustdoc-scrape-examples");

            if let Some(directory) = self.compiler_metrics_dir() {
                // Metrics setup must not prevent HTML from being generated. Collection
                // reports an unavailable metrics directory as its own output failure.
                match fs::create_dir_all(&directory) {
                    Ok(()) => {
                        command = command.rustdoc_arg("-Zmetrics-dir=/opt/rustwide/target/metrics");
                    }
                    Err(err) => error!(
                        ?err,
                        "cannot create metrics directory; building without metrics"
                    ),
                }
            }

            command
                .prepare()
                .map_err(BuildStepError::Prepare)?
                .run()
                .map_err(BuildStepError::Command)?;

            BuildStepError::as_output(|| {
                let output_dir = self.output_dir(target);

                let temp_dir = tempfile::Builder::new()
                    .prefix(&format!(
                        "{}.",
                        output_dir.file_stem().unwrap().to_string_lossy()
                    ))
                    .tempdir_in(&self.temp_dir()?)?;

                let destination = temp_dir.path().join("docs");
                fs::rename(&output_dir, &destination)
                    .context("couldn't move output dir to temp destination")?;

                Ok(HtmlOutput::new(temp_dir.keep().join("docs")))
            })
        })
    }

    /// Copy compiler metrics after HTML execution. Failure does not invalidate HTML.
    pub fn collect_compiler_metrics(&self) -> Result<Option<Vec<PathBuf>>> {
        let (Some(source), Some(destination)) = (
            self.compiler_metrics_dir(),
            self.environment.compiler_metrics_collection_path(),
        ) else {
            return Ok(None);
        };
        copy_compiler_metrics(&source, destination).map(Some)
    }

    fn compiler_metrics_dir(&self) -> Option<PathBuf> {
        self.environment
            .compiler_metrics_collection_path()
            .is_some()
            .then(|| self.build.host_target_dir().join("metrics"))
    }

    fn capture_rustwide_step<T>(
        &self,
        run: impl FnOnce() -> Result<T, BuildStepError>,
    ) -> StepResult<T> {
        capture_rustwide_step(self.limits.max_log_size(), run)
    }

    #[instrument(skip_all, fields(source_dir = %self.build.host_source_dir().display()))]
    fn regenerate_lockfile(&self) -> StepResult<()> {
        self.capture_rustwide_step(|| {
            let source_dir = self.build.host_source_dir();
            debug!("removing invalid lockfile");
            fs::remove_file(source_dir.join("Cargo.lock"))
                .context("removing invalid lockfile")
                .map_err(BuildStepError::Prepare)?;

            debug!("generating replacement lockfile");
            Command::new(
                self.environment.workspace(),
                self.environment.configured_toolchain().cargo(),
            )
            .current_directory(&source_dir)
            .arg("generate-lockfile")
            .run_capture()
            .map_err(BuildStepError::Command)?;

            debug!("fetching dependencies for replacement lockfile");
            Command::new(
                self.environment.workspace(),
                self.environment.configured_toolchain().cargo(),
            )
            .current_directory(source_dir)
            .args(["fetch", "--locked"])
            .run_capture()
            .map_err(BuildStepError::Command)?;

            debug!("refreshing Cargo metadata for the replacement lockfile");
            let metadata = read_cargo_metadata(self.environment, self.build)?;
            *self.cargo_metadata.borrow_mut() = metadata;

            debug!("replacement lockfile and metadata are ready");
            Ok(())
        })
    }
}

fn copy_compiler_metrics(source: &Path, destination: &Path) -> Result<Vec<PathBuf>> {
    let mut copied = Vec::new();
    copy_dir_all(source, destination, |path| copied.push(path.to_owned()))
        .context("copying compiler metrics")?;

    info!(
        file_count = copied.len(),
        source = %source.display(),
        dest = %destination.display(),
        dest_exists = destination.exists(),
        "found & copied files in compiler metric dir",
    );

    fs::remove_dir_all(source).context("removing compiler metrics directory")?;
    Ok(copied)
}

fn find_single_output_file(
    directory: impl AsRef<Path>,
    extension: impl AsRef<OsStr>,
) -> Result<PathBuf> {
    let directory = directory.as_ref();
    let extension = extension.as_ref();
    let matches: Vec<_> = fs::read_dir(directory)
        .with_context(|| format!("reading rustdoc output directory {}", directory.display()))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            let path = entry.path();
            path.extension()
                .is_some_and(|actual| actual.eq_ignore_ascii_case(extension))
                .then_some(path)
        })
        .collect();

    if matches.len() != 1 {
        bail!(
            "found {} instead of exactly one {} file in {}: {:?}",
            matches.len(),
            extension.to_string_lossy(),
            directory.display(),
            matches,
        );
    }

    Ok(matches.into_iter().next().expect("length checked above"))
}

#[cfg(test)]
mod tests {
    use docs_rs_types::ByteSizeExt as _;

    use super::*;
    use crate::StepResultExt as _;
    use std::ffi::OsStr;

    #[test]
    #[ignore = "requires Docker and a Rust toolchain"]
    fn refreshes_metadata_after_lockfile_regeneration() -> Result<()> {
        crate::logging::init(false);
        let workspace = crate::testing::test_workspace_path();
        let mut environment = BuildEnvironment::builder(workspace.as_path())
            .wait_for_workspace_lock(true)
            .fast_init(true)
            .validate_host_resources(false)
            .sandbox_image(crate::SandboxImageSource::linux_micro())
            .build()?;
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello-world");
        let krate = rustwide::Crate::local(&fixture);
        let release = environment
            .release(&krate)
            .run(|build| {
                assert!(build.cargo_metadata.borrow().root().description.is_none());
                let source = build.build.host_source_dir();
                let manifest = source.join("Cargo.toml");
                let contents = fs::read_to_string(&manifest)?;
                fs::write(
                    manifest,
                    contents.replace(
                        "[package]",
                        "[package]\ndescription = \"updated for retry\"",
                    ),
                )?;
                // The first attempt fails to parse the lockfile. Regeneration fixes
                // it, and must refresh the initially cached package metadata too.
                fs::write(source.join("Cargo.lock"), "[")?;
                Ok(build.build_docs())
            })?
            .into_inner();
        assert!(release.has_docs());
        assert!(
            release
                .default_target()
                .regenerate_lockfile()
                .unwrap()
                .is_ok()
        );
        assert_eq!(
            release.cargo_metadata().root().description.as_deref(),
            Some("updated for retry")
        );
        Ok(())
    }

    #[test_case::test_case(false; "target")]
    #[test_case::test_case(true; "release")]
    #[ignore = "requires Docker and a Rust toolchain"]
    fn retains_lockfile_regeneration_failure(build_release: bool) -> Result<()> {
        crate::logging::init(false);
        let workspace = crate::testing::test_workspace_path();
        let mut environment = BuildEnvironment::builder(workspace.as_path())
            .wait_for_workspace_lock(true)
            .fast_init(true)
            .validate_host_resources(false)
            .sandbox_image(crate::SandboxImageSource::linux_micro())
            .build()?;
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello-world");
        let krate = rustwide::Crate::local(&fixture);
        let target = environment
            .release(&krate)
            .run(|build| {
                // Corrupt the manifest after metadata loading to make HTML and
                // the ensuing generate-lockfile command fail independently.
                let source = build.build.host_source_dir();
                assert!(source.join("Cargo.lock").is_file());
                fs::write(source.join("Cargo.toml"), "[")?;
                Ok(if build_release {
                    let release = build.build_docs();
                    assert!(release.other_targets().is_empty());
                    release.default_target
                } else {
                    build
                        .build_target(HOST_TARGET)
                        .retry_without_lockfile(true)
                        .run()
                })
            })?
            .into_inner();
        let failure = target
            .regeneration_failure()
            .expect("regeneration failure retained");
        assert!(matches!(failure.value(), BuildStepError::Command(_)));
        assert!(!failure.duration.is_zero());
        assert!(failure.log().is_some_and(|log| log.contains("Cargo.toml")));
        assert!(target.documentation().is_err());
        assert!(
            target
                .documentation()
                .log()
                .is_some_and(|log| log.contains("Cargo.toml"))
        );
        assert!(target.duration() >= failure.duration + target.documentation().duration());
        Ok(())
    }

    #[test]
    fn metrics_copy_failure_preserves_source() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let source = temporary.path().join("metrics");
        fs::create_dir(&source)?;
        fs::write(source.join("metrics.json"), "{}")?;
        let destination = temporary.path().join("not-a-directory");
        fs::write(&destination, "")?;

        assert!(copy_compiler_metrics(&source, &destination).is_err());
        assert_eq!(fs::read_to_string(source.join("metrics.json"))?, "{}");
        Ok(())
    }

    #[test]
    fn capture_retains_preparation_failures_without_applying_policy() {
        crate::logging::init(false);
        let step = capture_rustwide_step::<()>(ByteSize::MAX, || {
            log::info!("installing additional target");
            Err(BuildStepError::Prepare(anyhow::anyhow!(
                "target unavailable"
            )))
        });
        let failure = step.unwrap_err();
        assert!(matches!(failure.value, BuildStepError::Prepare(_)));
        assert!(
            failure
                .log
                .as_deref()
                .unwrap()
                .contains("installing additional target")
        );
    }

    #[test]
    fn finds_exactly_one_output_file() -> Result<()> {
        let directory = tempfile::tempdir()?;
        fs::write(directory.path().join("crate.json"), "{}")?;
        fs::write(directory.path().join("index.html"), "")?;

        assert_eq!(
            find_single_output_file(directory.path(), OsStr::new("json"))?,
            directory.path().join("crate.json")
        );
        Ok(())
    }

    #[test]
    fn rejects_ambiguous_output_files() -> Result<()> {
        let directory = tempfile::tempdir()?;
        fs::write(directory.path().join("one.json"), "{}")?;
        fs::write(directory.path().join("two.json"), "{}")?;

        let error = find_single_output_file(directory.path(), "json").unwrap_err();
        assert!(error.to_string().contains("found 2 instead of exactly one"));
        Ok(())
    }
}
