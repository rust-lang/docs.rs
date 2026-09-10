use crate::{
    BuildEnvironment, BuildStepError, ReleaseBuildResult, RustdocJsonOutput, StepResult,
    TargetBuildResult, command::PrepareCommand, utils::copy_dir_all,
};
use anyhow::{Context as _, Result, bail};
use bon::bon;
use docs_rs_build_limits::Limits;
use docs_rs_cargo_metadata::CargoMetadata;
use docs_rs_types::doc_coverage::{self, DocCoverage};
use docsrs_metadata::{BuildTargets, Metadata};
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
use tracing::{Span, debug, instrument, warn};

/// Name of rustdoc's documentation output directory.
const DOC_OUTPUT_DIR_NAME: &str = "doc";

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

/// A prepared release inside an active rustwide sandbox.
pub struct ReleaseBuild<'build, 'ws> {
    pub(crate) environment: &'build BuildEnvironment,
    pub(crate) build: &'build Build<'ws>,
    pub(crate) metadata: Metadata,
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
        let metadata = Metadata::from_crate_root(build.host_source_dir())?;
        let resource_suffix = environment.resource_suffix()?;
        debug!(resource_suffix, "release build prepared");

        Ok(Self {
            environment,
            build,
            metadata,
            limits,
            resource_suffix,
            fetched_build_std_targets: RefCell::new(HashSet::new()),
        })
    }

    pub(crate) fn build_rustwide_command<'pl>(&self) -> Command<'ws, 'pl> {
        let mut command = self
            .build
            .cargo()
            .timeout(Some(self.limits.timeout()))
            .no_output_timeout(None);

        for (key, value) in self.metadata.environment_variables() {
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

    /// Return the host path containing documentation for a target.
    ///
    /// Cargo places proc-macro documentation in the host target directory even
    /// when a target argument is otherwise in use.
    pub fn output_dir(&self, target: &str) -> PathBuf {
        if self.metadata.proc_macro {
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
        self.metadata
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
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Limits applied to this release.
    pub fn limits(&self) -> &Limits {
        self.limits
    }

    /// Build coverage, rustdoc JSON, and HTML for the full docs.rs target set.
    ///
    /// All commands execute through the same rustwide build and reusable
    /// sandbox. Coverage and JSON failures are returned with their individual
    /// steps and do not prevent the primary HTML build from running.
    #[instrument(skip_all, fields(crate_name, crate_version))]
    pub fn build_docs(&self) -> Result<ReleaseBuildResult> {
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

        let cargo_metadata = self.load_cargo_metadata()?;
        let root_package = cargo_metadata.root();
        Span::current()
            .record("crate_name", root_package.name.as_str())
            .record(
                "crate_version",
                tracing::field::display(&root_package.version),
            );

        let default_target_result = self
            .build_target(default_target)
            .retry_without_lockfile(true)
            .run()?;

        let default_has_docs = cargo_metadata
            .root()
            .library_name()
            .is_some_and(|name| default_target_result.has_docs(&name));

        let mut target_results = vec![default_target_result];

        if default_has_docs {
            for target in other_targets {
                target_results.push(self.build_target(target).run()?);
            }
        } else {
            debug!("default target produced no library documentation; skipping other targets");
        }

        Ok(ReleaseBuildResult {
            statistics: self.build.statistics(),
            metadata: self.metadata.clone(),
            cargo_metadata,
            targets: target_results,
        })
    }

    /// Build coverage, rustdoc JSON, and HTML for one target.
    #[builder(finish_fn(name=run))]
    pub fn build_target(
        &self,
        #[builder(start_fn)] target: &str,
        #[builder(default = false)] retry_without_lockfile: bool,
    ) -> Result<TargetBuildResult> {
        let started = Instant::now();
        let is_default = target == self.metadata_targets().default_target;
        let mut target_result = self.build_target_once(target, is_default)?;

        if retry_without_lockfile
            && matches!(
                target_result.documentation.outcome,
                Err(BuildStepError::Command(_))
            )
            && self.build.host_source_dir().join("Cargo.lock").exists()
        {
            debug!(
                target,
                "target build failed; retrying with a regenerated lockfile"
            );
            self.regenerate_lockfile()?;
            target_result = self.build_target_once(target, is_default)?;
        }

        target_result.duration = started.elapsed();
        Ok(target_result)
    }

    #[instrument(skip_all)]
    fn build_target_once(&self, target: &str, is_default: bool) -> Result<TargetBuildResult> {
        // Coverage must precede the HTML build because Cargo currently clears
        // rustdoc's target output directory between these invocations.
        let coverage_result = self.build_coverage(target).abort_on_prepare(is_default)?;
        let rustdoc_json_result = self
            .build_rustdoc_json(target)
            .abort_on_prepare(is_default)?;
        let documentation_result = self
            .build_documentation(target)
            .abort_on_prepare(is_default)?;
        let compiler_metrics = self.collect_compiler_metrics();

        if documentation_result.successful() && self.metadata.proc_macro {
            debug_assert!(is_default, "proc macros only support their host target");
        }

        debug!(
            target,
            coverage_successful = coverage_result.successful(),
            rustdoc_json_successful = rustdoc_json_result.successful(),
            documentation_successful = documentation_result.successful(),
            compiler_metrics_successful = compiler_metrics.successful(),
            "target build completed"
        );

        Ok(TargetBuildResult {
            duration: std::time::Duration::ZERO,
            target: target.into(),
            is_default,
            documentation: documentation_result,
            rustdoc_json: rustdoc_json_result,
            coverage: coverage_result,
            compiler_metrics,
        })
    }

    /// Collect documentation coverage for one target.
    ///
    /// All failures retain their duration and log; the caller decides whether to abort.
    #[instrument(skip_all, fields(target))]
    pub fn build_coverage(&self, target: &str) -> StepResult<Option<DocCoverage>> {
        Self::capture_step(self.limits.max_log_size(), || {
            let mut coverage = DocCoverage::default();
            self.command(target)
                .rustdoc_args(["--output-format", "json", "--show-coverage"])
                .prepare()
                .map_err(BuildStepError::Prepare)?
                .log_output(true)
                .run()
                .map_err(BuildStepError::Command)?;

            let output_dir = self.output_dir(target);
            let path = find_single_output_file(&output_dir, "json")?;
            let reader = BufReader::new(File::open(path).map_err(anyhow::Error::from)?);
            for line in reader.lines() {
                let line = line.map_err(anyhow::Error::from)?;
                coverage
                    .extend(doc_coverage::parse_line(&line).context("parsing coverage output")?);
            }

            Ok((coverage.total_items != 0 || coverage.documented_items != 0).then_some(coverage))
        })
    }

    /// Build unstable rustdoc JSON for one target.
    ///
    /// All failures retain their duration and log; the caller decides whether to abort.
    #[instrument(skip_all, fields(target))]
    pub fn build_rustdoc_json(&self, target: &str) -> StepResult<RustdocJsonOutput> {
        Self::capture_step(self.limits.max_log_size(), || {
            self.command(target)
                .rustdoc_args(["--output-format", "json"])
                .prepare()
                .map_err(BuildStepError::Prepare)?
                .run()
                .map_err(BuildStepError::Command)?;

            find_single_output_file(self.output_dir(target), "json")
                .map(RustdocJsonOutput::new)
                .map_err(Into::into)
        })
    }

    /// Build HTML documentation without emitting shared static files.
    ///
    /// All failures retain their duration and log; the caller decides whether to abort.
    #[instrument(skip_all)]
    pub fn build_documentation(&self, target: &str) -> StepResult<PathBuf> {
        self.build_html(target, Emit::HtmlNonStaticFiles)
    }

    #[instrument(skip_all)]
    pub(crate) fn build_essential_files(&self) -> Result<PathBuf> {
        let output = self
            .build_html(docsrs_metadata::HOST_TARGET, Emit::HtmlStaticFiles)
            .into_result()?;

        let static_files = output.join("static.files");
        if !static_files.is_dir() {
            bail!(
                "essential-files build did not produce {}",
                static_files.display()
            );
        }
        Ok(static_files)
    }

    #[instrument(skip_all, fields(target, emit))]
    fn build_html(&self, target: &str, emit: Emit) -> StepResult<PathBuf> {
        Self::capture_step(self.limits.max_log_size(), || {
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
                    Err(error) => warn!(
                        ?error,
                        "cannot create metrics directory; building without metrics"
                    ),
                }
            }
            command
                .prepare()
                .map_err(BuildStepError::Prepare)?
                .run()
                .map_err(BuildStepError::Command)?;
            Ok(self.output_dir(target))
        })
    }

    /// Copy compiler metrics after HTML execution. Failure does not invalidate HTML.
    pub fn collect_compiler_metrics(&self) -> StepResult<Vec<PathBuf>> {
        Self::capture_step(self.limits.max_log_size(), || {
            let (Some(source), Some(destination)) = (
                self.compiler_metrics_dir(),
                self.environment.compiler_metrics_collection_path(),
            ) else {
                return Ok(Vec::new());
            };
            copy_compiler_metrics(&source, destination).map_err(BuildStepError::Output)
        })
    }

    fn compiler_metrics_dir(&self) -> Option<PathBuf> {
        self.environment
            .compiler_metrics_collection_path()
            .is_some()
            .then(|| self.build.host_target_dir().join("metrics"))
    }

    fn capture_step<T>(
        max_log_size: usize,
        run: impl FnOnce() -> Result<T, BuildStepError>,
    ) -> StepResult<T> {
        let mut storage = LogStorage::new(log::LevelFilter::Info);
        storage.set_max_size(max_log_size);
        let started = Instant::now();
        let outcome = logging::capture(&storage, run);
        StepResult {
            outcome,
            duration: started.elapsed(),
            log: storage.to_string(),
        }
    }

    #[instrument(skip_all, fields(source_dir = %self.build.host_source_dir().display()))]
    fn regenerate_lockfile(&self) -> Result<()> {
        Self::capture_step(self.limits.max_log_size(), || {
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
            .context("generating a replacement lockfile")
            .map_err(BuildStepError::Prepare)?;

            debug!("fetching dependencies for replacement lockfile");
            Command::new(
                self.environment.workspace(),
                self.environment.configured_toolchain().cargo(),
            )
            .current_directory(source_dir)
            .args(["fetch", "--locked"])
            .run_capture()
            .context("fetching dependencies for the replacement lockfile")
            .map_err(BuildStepError::Prepare)?;

            debug!("replacement lockfile is ready");
            Ok(())
        })
        .into_result()?;
        Ok(())
    }

    #[instrument(skip_all)]
    fn load_cargo_metadata(&self) -> Result<CargoMetadata> {
        self.environment
            .load_cargo_metadata(self.build.host_source_dir())
    }
}

fn copy_compiler_metrics(source: &Path, destination: &Path) -> Result<Vec<PathBuf>> {
    let mut copied = Vec::new();
    copy_dir_all(source, destination, |path| copied.push(path.to_owned()))
        .context("copying compiler metrics")?;
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
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn preparation_aborts_with_diagnostics_but_command_and_output_failures_continue() {
        crate::logging::init(false);
        for error in [
            BuildStepError::Command(rustwide::cmd::CommandError::Timeout(1)),
            BuildStepError::Output(anyhow::anyhow!("invalid JSON")),
        ] {
            let step = ReleaseBuild::capture_step::<()>(1024, || Err(error))
                .abort_on_prepare(true)
                .unwrap();
            assert!(!step.successful());
        }
        let started = Instant::now();
        let step = ReleaseBuild::capture_step::<()>(1024, || {
            log::info!("fetching build-std dependencies");
            Err(BuildStepError::Prepare(anyhow::anyhow!(
                "dependency download failed"
            )))
        });
        assert!(step.duration > std::time::Duration::ZERO);
        assert!(step.duration <= started.elapsed());
        let error = step.abort_on_prepare(true).unwrap_err();
        let failure = error.downcast_ref::<crate::FailedStep>().unwrap();
        assert!(failure.log.contains("fetching build-std dependencies"));
        assert!(matches!(failure.error, BuildStepError::Prepare(_)));
    }

    #[test]
    fn metrics_copy_failure_is_nonfatal() -> Result<()> {
        crate::logging::init(false);
        let temporary = tempfile::tempdir()?;
        let source = temporary.path().join("metrics");
        fs::create_dir(&source)?;
        fs::write(source.join("metrics.json"), "{}")?;
        let destination = temporary.path().join("not-a-directory");
        fs::write(&destination, "")?;
        let metrics = ReleaseBuild::capture_step(1024, || {
            copy_compiler_metrics(&source, &destination).map_err(BuildStepError::Output)
        })
        .abort_on_prepare(true)?;
        assert!(matches!(metrics.outcome, Err(BuildStepError::Output(_))));
        Ok(())
    }

    #[test]
    fn additional_target_preparation_failures_remain_step_results() {
        crate::logging::init(false);
        let step = ReleaseBuild::capture_step::<()>(1024, || {
            log::info!("installing additional target");
            Err(BuildStepError::Prepare(anyhow::anyhow!(
                "target unavailable"
            )))
        })
        .abort_on_prepare(false)
        .unwrap();
        assert!(matches!(step.outcome, Err(BuildStepError::Prepare(_))));
        assert!(!step.successful());
        assert!(step.log.contains("installing additional target"));
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
