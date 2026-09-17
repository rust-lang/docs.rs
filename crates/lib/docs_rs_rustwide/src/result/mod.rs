pub(crate) mod step;

use anyhow::{Context as _, Result};
use docs_rs_cargo_metadata::CargoMetadata;
use docs_rs_rustdoc_json::{RustdocJsonFormatVersion, read_format_version_from_rustdoc_json};
use docs_rs_types::{Duration, doc_coverage::DocCoverage};
use docsrs_metadata::Metadata;
use rustwide::SandboxStatistics;
use std::{
    fs::File,
    iter,
    path::{Path, PathBuf},
};
use step::{StepFailure, StepResult, StepResultExt as _};
use tracing::{debug, instrument};

/// Output of a completed release lifecycle, including fetch and sandbox cleanup.
pub struct BuildResult<T> {
    pub(crate) inner: rustwide::BuildResult<T>,
    pub(crate) duration: Duration,
}

impl<T> BuildResult<T> {
    /// Elapsed time from entering fetch through sandbox teardown and cache cleanup.
    /// Includes caller work between fetch and run, but excludes workspace/toolchain setup.
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// Final statistics for the shared sandbox.
    pub fn statistics(&self) -> &SandboxStatistics {
        self.inner.statistics()
    }

    /// Return the callback output, discarding lifecycle duration and final statistics.
    pub fn into_inner(self) -> T {
        self.inner.into_inner()
    }
}

/// HTML artifacts retained until the enclosing Rustwide build directory is cleaned.
/// Dropping this value does not remove the files.
#[derive(Clone, Debug)]
pub struct HtmlOutput {
    pub(crate) path: PathBuf,
}

impl AsRef<Path> for HtmlOutput {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

impl HtmlOutput {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Path to the generated HTML documentation directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the HTML documentation output directory exists.
    pub fn exists(&self) -> bool {
        self.path.is_dir()
    }

    /// Whether documentation exists for the crate's library target.
    pub fn has_docs(&self, library_name: &str) -> bool {
        self.path.join(library_name).is_dir()
    }
}

/// A rustdoc JSON artifact produced by a successful JSON build.
///
/// Retained until the enclosing Rustwide build directory is cleaned.
/// Dropping this value does not remove the file.
#[derive(Clone, Debug)]
pub struct RustdocJsonOutput {
    path: PathBuf,
}

impl AsRef<Path> for RustdocJsonOutput {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

impl RustdocJsonOutput {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Path to the generated rustdoc JSON file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the format version embedded in the rustdoc JSON file.
    ///
    /// Parsing is lazy so callers that only need the artifact do not pay this cost.
    #[instrument(skip_all)]
    pub fn format_version(&self) -> Result<RustdocJsonFormatVersion> {
        let path = self.path();
        debug!(
            path = %path.display(),
            "reading rustdoc JSON format version"
        );
        let file = File::open(path)
            .with_context(|| format!("opening rustdoc JSON at {}", path.display()))?;
        let version = read_format_version_from_rustdoc_json(file)
            .with_context(|| format!("reading format version from {}", path.display()))?;
        debug!(?version, "read rustdoc JSON format version");
        Ok(version)
    }
}

/// Results for all build modes of one compilation target.
#[derive(Debug)]
pub struct TargetBuildResult {
    pub(crate) duration: Option<Duration>,
    /// Rust target triple.
    pub(crate) target: String,
    /// Whether this is the release's default target.
    pub(crate) is_default: bool,
    /// HTML documentation output directory.
    pub(crate) documentation: StepResult<HtmlOutput>,
    /// Rustdoc JSON build result.
    pub(crate) rustdoc_json: StepResult<RustdocJsonOutput>,
    /// Documentation coverage build result.
    pub(crate) coverage: StepResult<Option<DocCoverage>>,
    /// Compiler metrics files copied out of this target's HTML build.
    pub(crate) compiler_metrics: Option<Vec<PathBuf>>,
    /// optionally regenerate lockfile
    pub(crate) regenerate_lockfile: Option<StepResult<()>>,
}

impl TargetBuildResult {
    /// Whether this is the release's default target.
    pub fn is_default(&self) -> bool {
        self.is_default
    }

    /// Elapsed time for this target, including all attempts and lockfile regeneration.
    pub fn duration(&self) -> Duration {
        self.duration
            .expect("when library users access the duration, we always have one")
    }

    pub fn target(&self) -> &str {
        self.target.as_str()
    }

    /// Whether the primary HTML step succeeded, including output collection.
    ///
    /// This does not require a documentation directory to exist; use
    /// [`Self::documentation_succeeded`] or [`Self::has_docs`] to check for output.
    pub fn build_succeeded(&self) -> bool {
        self.documentation.is_ok()
    }

    /// Whether the primary HTML documentation build completed and produced output.
    pub fn documentation_succeeded(&self) -> bool {
        self.documentation()
            .as_inner()
            .is_ok_and(HtmlOutput::exists)
    }

    /// Whether this target produced documentation for the crate's library target.
    pub fn has_docs(&self, library_name: &str) -> bool {
        self.documentation()
            .as_inner()
            .is_ok_and(|html| html.has_docs(library_name))
    }

    pub fn coverage(&self) -> &StepResult<Option<DocCoverage>> {
        &self.coverage
    }

    pub fn documentation(&self) -> &StepResult<HtmlOutput> {
        &self.documentation
    }

    pub fn compiler_metrics(&self) -> Option<&[PathBuf]> {
        self.compiler_metrics.as_deref()
    }

    pub fn rustdoc_json(&self) -> &StepResult<RustdocJsonOutput> {
        &self.rustdoc_json
    }

    /// Failure that prevented retrying this target with a regenerated lockfile.
    pub fn regeneration_failure(&self) -> Option<&StepFailure> {
        self.regenerate_lockfile.as_ref()?.as_ref().err()
    }

    pub fn regenerate_lockfile(&self) -> Option<&StepResult<()>> {
        self.regenerate_lockfile.as_ref()
    }
}

/// Service-independent result of building one crate release.
pub struct ReleaseBuildResult {
    /// Sandbox statistics captured after all documentation targets finished.
    /// Includes all targets and retry attempts in the shared sandbox.
    pub(crate) statistics: SandboxStatistics,
    /// Metadata read from rustwide's prepared source directory.
    pub(crate) docsrs_metadata: Metadata,
    /// Cargo's resolved package metadata for the prepared source.
    pub(crate) cargo_metadata: CargoMetadata,
    pub(crate) default_target: TargetBuildResult,
    pub(crate) other_targets: Vec<TargetBuildResult>,
}

impl ReleaseBuildResult {
    pub fn statistics(&self) -> &SandboxStatistics {
        &self.statistics
    }

    pub fn docsrs_metadata(&self) -> &Metadata {
        &self.docsrs_metadata
    }

    pub fn cargo_metadata(&self) -> &CargoMetadata {
        &self.cargo_metadata
    }

    pub fn other_targets(&self) -> &[TargetBuildResult] {
        &self.other_targets
    }

    /// Whether the default target's HTML step succeeded, including output collection.
    ///
    /// This does not require a documentation directory to exist; use
    /// [`Self::documentation_succeeded`] or [`Self::has_docs`] to check for output.
    pub fn build_succeeded(&self) -> bool {
        self.default_target.build_succeeded()
    }

    /// Whether the default HTML documentation build completed and produced output.
    pub fn documentation_succeeded(&self) -> bool {
        self.default_target.documentation_succeeded()
    }

    /// Whether the default target produced documentation for this crate's library target.
    pub fn has_docs(&self) -> bool {
        self.cargo_metadata
            .root()
            .library_name()
            .is_some_and(|name| self.default_target.has_docs(&name))
    }

    pub fn default_target(&self) -> &TargetBuildResult {
        &self.default_target
    }

    pub fn targets(&self) -> impl Iterator<Item = &TargetBuildResult> {
        iter::once(&self.default_target).chain(self.other_targets.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BuildStepError, StepReport};
    use docs_rs_types::BuildError as _;
    use rustwide::cmd::CommandError;
    use std::fs;
    use test_case::test_case;

    #[test_case(BuildStepError::Command(CommandError::Timeout(1)), "Timeout"; "timeout")]
    #[test_case(BuildStepError::Command(CommandError::SandboxOOM), "SandboxOOM"; "sandbox oom")]
    #[test_case(BuildStepError::Prepare(anyhow::anyhow!("target unavailable")), "InternalPrepare"; "preparation")]
    #[test_case(BuildStepError::Output(anyhow::anyhow!("invalid output")), "InternalOutput"; "output processing")]
    fn classifies_build_step_errors(error: BuildStepError, expected: &str) {
        assert_eq!(error.kind(), expected);
    }

    fn target_result(html_output: HtmlOutput) -> TargetBuildResult {
        let dummy_json_filename = html_output.path().with_file_name("dummy.json");
        fs::write(&dummy_json_filename, b"{}").unwrap();

        TargetBuildResult {
            target: "x86_64-unknown-linux-gnu".into(),
            is_default: true,
            duration: Some(Duration::ZERO),
            compiler_metrics: None,
            documentation: Ok(StepReport {
                value: html_output,
                log: None,
                duration: Duration::ZERO,
            }),
            rustdoc_json: Ok(StepReport {
                value: RustdocJsonOutput::new(dummy_json_filename.to_path_buf()),
                log: None,
                duration: Duration::ZERO,
            }),
            coverage: Ok(StepReport {
                value: None,
                log: None,
                duration: Duration::ZERO,
            }),
            regenerate_lockfile: None,
        }
    }

    #[test]
    fn documentation_success_requires_documentation_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("docs");
        let result = target_result(HtmlOutput::new(path.clone()));

        assert!(!result.documentation().as_inner().unwrap().exists());
        assert!(result.build_succeeded());
        assert!(!result.documentation_succeeded());
        assert!(!result.has_docs("example_crate"));

        fs::create_dir(&path).unwrap();

        assert!(result.documentation().as_inner().unwrap().exists());
        assert!(result.build_succeeded());
        assert!(result.documentation_succeeded());
        assert!(!result.has_docs("example_crate"));

        fs::create_dir(path.join("example_crate")).unwrap();

        assert!(
            result
                .documentation()
                .as_inner()
                .unwrap()
                .has_docs("example_crate")
        );
        assert!(result.has_docs("example_crate"));
    }

    #[test]
    fn reads_rustdoc_json_format_version_lazily() -> Result<()> {
        let path = tempfile::NamedTempFile::new()?;
        fs::write(&path, r#"{"format_version":42}"#)?;
        let output = RustdocJsonOutput::new(path.path().to_owned());

        assert_eq!(
            output.format_version()?,
            RustdocJsonFormatVersion::Version(42)
        );
        Ok(())
    }

    #[test_case("not JSON"; "malformed json")]
    #[test_case("{}"; "missing format version")]
    fn reports_invalid_rustdoc_json_metadata(contents: &str) -> Result<()> {
        let path = tempfile::NamedTempFile::new()?;
        fs::write(&path, contents)?;
        let output = RustdocJsonOutput::new(path.path().to_owned());

        assert!(output.format_version().is_err());
        Ok(())
    }
}
