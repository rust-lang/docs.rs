use crate::{
    BuildEnvironment, BuildStepError, ReleaseBuildResult, RustdocJsonOutput, StepResult,
    TargetBuildResult, command::PrepareCommand,
};
use anyhow::{Context as _, Result, bail};
use bon::bon;
use docs_rs_build_limits::Limits;
use docs_rs_cargo_metadata::CargoMetadata;
use docs_rs_types::doc_coverage::{self, DocCoverage};
use docsrs_metadata::{BuildTargets, Metadata};
use rustwide::Build;
use rustwide::{
    cmd::Command,
    logging::{self, LogStorage},
};
use std::{
    cell::RefCell,
    collections::HashSet,
    ffi::OsStr,
    fs::{self, File},
    io::{BufRead as _, BufReader},
    iter,
    path::{Path, PathBuf},
};
use tracing::{debug, instrument, warn};

/// Name of rustdoc's documentation output directory.
pub const DOC_OUTPUT_DIR_NAME: &str = "doc";

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

/// A prepared release inside an active rustwide sandbox.
pub struct ReleaseBuild<'build, 'ws> {
    pub(crate) environment: &'build BuildEnvironment,
    pub(crate) build: &'build Build<'ws>,
    pub(crate) metadata: Metadata,
    pub(crate) limits: &'build Limits,
    pub(crate) resource_suffix: String,
    fetched_build_std_targets: RefCell<HashSet<String>>,
    compiler_metrics: RefCell<Vec<PathBuf>>,
}

#[bon]
impl<'build, 'ws> ReleaseBuild<'build, 'ws> {
    #[instrument(
        skip(environment, build, limits),
        fields(source_dir = %build.host_source_dir().display())
    )]
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
            compiler_metrics: RefCell::new(Vec::new()),
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
    #[instrument(skip(self, targets))]
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
    #[instrument(skip(self))]
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

        self.fetch_build_std_dependencies(
            iter::once(default_target).chain(other_targets.iter().copied()),
        )?;

        let cargo_metadata = self.load_cargo_metadata()?;

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
        let is_default = target == self.metadata_targets().default_target;
        let mut target_result = self.build_target_once(target, is_default);

        if retry_without_lockfile
            && !target_result.successful()
            && self.build.host_source_dir().join("Cargo.lock").exists()
        {
            debug!(
                target,
                "target build failed; retrying with a regenerated lockfile"
            );
            self.regenerate_lockfile()?;
            target_result = self.build_target_once(target, is_default);
        }

        Ok(target_result)
    }

    #[instrument(skip(self), fields(target, is_default))]
    fn build_target_once(&self, target: &str, is_default: bool) -> TargetBuildResult {
        self.compiler_metrics.borrow_mut().clear();
        // Coverage must precede the HTML build because Cargo currently clears
        // rustdoc's target output directory between these invocations.
        let coverage_result = self.build_coverage(target);
        let rustdoc_json_result = self.build_rustdoc_json(target);
        let documentation_result = self.build_documentation(target);
        let compiler_metrics = self.compiler_metrics.take();

        if documentation_result.successful() && self.metadata.proc_macro {
            debug_assert!(is_default, "proc macros only support their host target");
        }

        debug!(
            coverage_successful = coverage_result.successful(),
            rustdoc_json_successful = rustdoc_json_result.successful(),
            documentation_successful = documentation_result.successful(),
            compiler_metrics_count = compiler_metrics.len(),
            "target build completed"
        );

        TargetBuildResult {
            target: target.into(),
            is_default,
            documentation: documentation_result,
            rustdoc_json: rustdoc_json_result,
            coverage: coverage_result,
            compiler_metrics,
        }
    }

    /// Collect documentation coverage for one target.
    #[instrument(skip(self), fields(target))]
    pub fn build_coverage(&self, target: &str) -> StepResult<Option<DocCoverage>> {
        self.capture_step(|| {
            let mut coverage = DocCoverage::default();
            self.command(target)
                .rustdoc_args(["--output-format", "json", "--show-coverage"])
                .prepare()
                .map_err(BuildStepError::Output)?
                .log_output(true)
                .run()
                .map_err(BuildStepError::Command)?;

            let output_dir = self.output_dir(target);
            if let Ok(path) = find_single_output_file(&output_dir, "json") {
                let reader = BufReader::new(File::open(path).map_err(anyhow::Error::from)?);
                for line in reader.lines() {
                    let line = line.map_err(anyhow::Error::from)?;
                    match doc_coverage::parse_line(&line) {
                        Ok(file_coverages) => coverage.extend(file_coverages),
                        Err(error) => warn!(?error, line, "failed to parse coverage line"),
                    }
                }
            }

            Ok((coverage.total_items != 0 || coverage.documented_items != 0).then_some(coverage))
        })
    }

    /// Build unstable rustdoc JSON for one target.
    #[instrument(skip(self), fields(target))]
    pub fn build_rustdoc_json(&self, target: &str) -> StepResult<RustdocJsonOutput> {
        self.capture_step(|| {
            self.command(target)
                .rustdoc_args(["--output-format", "json"])
                .prepare()
                .map_err(BuildStepError::Output)?
                .run()
                .map_err(BuildStepError::Command)?;

            find_single_output_file(self.output_dir(target), "json")
                .map(RustdocJsonOutput::new)
                .map_err(BuildStepError::Output)
        })
    }

    /// Build HTML documentation without emitting shared static files.
    #[instrument(skip(self), fields(target))]
    pub fn build_documentation(&self, target: &str) -> StepResult<PathBuf> {
        self.build_html(target, Emit::HtmlNonStaticFiles)
    }

    #[instrument(skip(self))]
    pub(crate) fn build_essential_files(&self) -> Result<PathBuf> {
        let result = self.build_html(docsrs_metadata::HOST_TARGET, Emit::HtmlStaticFiles);
        if let Some(error) = result.error {
            bail!(
                "failed to build shared rustdoc static files: {error}\n{}",
                result.log
            );
        }

        let output = result
            .output
            .context("essential-files build succeeded without an output directory")?;
        essential_files_directory(&output)
    }

    #[instrument(skip(self), fields(target, emit = emit.as_str()))]
    fn build_html(&self, target: &str, emit: Emit) -> StepResult<PathBuf> {
        self.capture_step(|| {
            let metrics_dir = self.compiler_metrics_dir();
            if let Some(metrics_dir) = &metrics_dir {
                fs::create_dir_all(metrics_dir).map_err(anyhow::Error::from)?;
            }

            let mut command = self
                .command(target)
                .rustdoc_arg(format!("--emit={}", emit.as_str()))
                .rustdoc_args(["--resource-suffix", &self.resource_suffix])
                .cargo_arg("-Zrustdoc-scrape-examples");
            if metrics_dir.is_some() {
                command = command.cargo_args([
                    "--config".to_owned(),
                    "build.rustdocflags=['-Zmetrics-dir=/opt/rustwide/target/metrics']".to_owned(),
                ]);
            }
            let command_result = command
                .prepare()
                .map_err(BuildStepError::Output)?
                .run()
                .map_err(BuildStepError::Command);

            if let (Some(source), Some(destination)) = (
                metrics_dir,
                self.environment.compiler_metrics_collection_path(),
            ) {
                let copied_metrics = copy_directory_contents(&source, destination)?;
                debug!(
                    count = copied_metrics.len(),
                    destination = %destination.display(),
                    "compiler metrics collected"
                );
                self.compiler_metrics.borrow_mut().extend(copied_metrics);
                fs::remove_dir_all(source).map_err(anyhow::Error::from)?;
            }

            command_result?;

            Ok(self.output_dir(target))
        })
    }

    fn compiler_metrics_dir(&self) -> Option<PathBuf> {
        self.environment
            .compiler_metrics_collection_path()
            .is_some()
            .then(|| self.build.host_target_dir().join("metrics"))
    }

    fn capture_step<T>(&self, run: impl FnOnce() -> Result<T, BuildStepError>) -> StepResult<T> {
        let mut storage = LogStorage::new(log::LevelFilter::Info);
        storage.set_max_size(self.limits.max_log_size());
        let captured_result = logging::capture(&storage, run);

        match captured_result {
            Ok(output) => StepResult {
                output: Some(output),
                error: None,
                log: storage.to_string(),
            },
            Err(error) => StepResult {
                output: None,
                error: Some(error),
                log: storage.to_string(),
            },
        }
    }

    #[instrument(skip(self), fields(source_dir = %self.build.host_source_dir().display()))]
    fn regenerate_lockfile(&self) -> Result<()> {
        let source_dir = self.build.host_source_dir();
        debug!("removing invalid lockfile");
        fs::remove_file(source_dir.join("Cargo.lock"))?;

        debug!("generating replacement lockfile");
        Command::new(
            self.environment.workspace(),
            self.environment.configured_toolchain().cargo(),
        )
        .current_directory(&source_dir)
        .arg("generate-lockfile")
        .run_capture()
        .context("generating a replacement lockfile")?;

        debug!("fetching dependencies for replacement lockfile");
        Command::new(
            self.environment.workspace(),
            self.environment.configured_toolchain().cargo(),
        )
        .current_directory(source_dir)
        .args(["fetch", "--locked"])
        .run_capture()
        .context("fetching dependencies for the replacement lockfile")?;

        debug!("replacement lockfile is ready");
        Ok(())
    }

    #[instrument(skip(self), fields(source_dir = %self.build.host_source_dir().display()))]
    fn load_cargo_metadata(&self) -> Result<CargoMetadata> {
        debug!("loading Cargo metadata");
        let output = Command::new(
            self.environment.workspace(),
            self.environment.configured_toolchain().cargo(),
        )
        .args(["metadata", "--format-version", "1"])
        .current_directory(self.build.host_source_dir())
        .log_output(false)
        .run_capture()?;

        let [metadata] = output.stdout_lines() else {
            bail!("invalid output returned by `cargo metadata`");
        };

        let metadata = CargoMetadata::load_from_metadata(metadata)?;
        debug!("Cargo metadata loaded");
        Ok(metadata)
    }
}

fn copy_directory_contents(source: &Path, destination: &Path) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(destination)?;
    let mut copied = Vec::new();
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copied.extend(copy_directory_contents(&source_path, &destination_path)?);
        } else {
            fs::copy(source_path, &destination_path)?;
            copied.push(destination_path);
        }
    }
    Ok(copied)
}

fn essential_files_directory(documentation_output: &Path) -> Result<PathBuf> {
    let static_files = documentation_output.join("static.files");
    if !static_files.is_dir() {
        bail!(
            "essential-files build did not produce {}",
            static_files.display()
        );
    }
    Ok(static_files)
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

    #[test]
    fn requires_static_files_directory_for_essential_files() -> Result<()> {
        let directory = tempfile::tempdir()?;
        assert!(essential_files_directory(directory.path()).is_err());

        let static_files = directory.path().join("static.files");
        fs::create_dir(&static_files)?;
        assert_eq!(essential_files_directory(directory.path())?, static_files);
        Ok(())
    }

    #[test]
    fn reports_copied_compiler_metrics_paths() -> Result<()> {
        let source = tempfile::tempdir()?;
        let destination = tempfile::tempdir()?;
        fs::create_dir(source.path().join("nested"))?;
        fs::write(source.path().join("crate-1.json"), "{}")?;
        fs::write(source.path().join("nested/crate-2.json"), "{}")?;

        let mut copied = copy_directory_contents(source.path(), destination.path())?;
        copied.sort();
        let mut expected = vec![
            destination.path().join("crate-1.json"),
            destination.path().join("nested/crate-2.json"),
        ];
        expected.sort();
        assert_eq!(copied, expected);
        Ok(())
    }
}
