pub(crate) mod step;

use anyhow::{Context as _, Result};
use docs_rs_cargo_metadata::CargoMetadata;
use docs_rs_rustdoc_json::{RustdocJsonFormatVersion, read_format_version_from_rustdoc_json};
use docs_rs_types::doc_coverage::DocCoverage;
use docsrs_metadata::Metadata;
use rustwide::SandboxStatistics;
use std::{
    fs::File,
    iter,
    path::{Path, PathBuf},
    time::Duration,
};
use step::{BuildStepError, StepResult};
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

/// A rustdoc JSON artifact produced by a successful JSON build.
#[derive(Clone, Debug)]
pub struct RustdocJsonOutput {
    path: PathBuf,
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
        debug!(
            path = %self.path.display(),
            "reading rustdoc JSON format version"
        );
        let file = File::open(&self.path)
            .with_context(|| format!("opening rustdoc JSON at {}", self.path.display()))?;
        let version = read_format_version_from_rustdoc_json(file)
            .with_context(|| format!("reading format version from {}", self.path.display()))?;
        debug!(?version, "read rustdoc JSON format version");
        Ok(version)
    }
}

/// Results for all build modes of one compilation target.
#[derive(Debug)]
pub struct TargetBuildResult {
    pub(crate) duration: Option<Duration>,
    /// Rust target triple.
    pub target: String,
    /// is the target the default target
    pub is_default: bool,
    /// HTML documentation output directory.
    pub documentation: StepResult<PathBuf>,
    /// Rustdoc JSON build result.
    pub rustdoc_json: StepResult<RustdocJsonOutput>,
    /// Documentation coverage build result.
    pub coverage: StepResult<Option<DocCoverage>>,
    /// Compiler metrics files copied out of this target's HTML build.
    pub compiler_metrics: Option<Vec<PathBuf>>,
    /// optionally regenerate lockfile
    pub regenerate_lockfile: Option<StepResult<()>>,
}

impl TargetBuildResult {
    /// Elapsed time for this target, including all attempts and lockfile regeneration.
    pub fn duration(&self) -> Duration {
        self.duration
            .expect("when library users access the duration, we always have one")
    }

    /// Whether rustdoc produced a documentation output directory.
    ///
    /// Cargo can exit successfully without generating documentation for a
    /// target, so command success alone is not sufficient.
    pub fn documentation_exists(&self) -> bool {
        self.documentation
            .as_ref()
            .is_ok_and(|report| report.value.is_dir())
    }

    /// Whether Cargo completed the primary HTML documentation command successfully.
    pub fn build_succeeded(&self) -> bool {
        self.documentation.is_ok()
    }

    /// Whether the primary HTML documentation build completed and produced output.
    pub fn documentation_succeeded(&self) -> bool {
        self.build_succeeded() && self.documentation_exists()
    }

    /// Whether this target produced documentation for the crate's library target.
    pub fn has_docs(&self, library_name: &str) -> bool {
        self.documentation_succeeded()
            && self
                .documentation
                .as_ref()
                .is_ok_and(|report| report.value.join(library_name).is_dir())
    }

    pub fn coverage(&self) -> Result<Option<&DocCoverage>, &BuildStepError> {
        self.coverage
            .as_ref()
            .map(|report| report.value.as_ref())
            .map_err(|report| &report.value)
    }

    pub fn documentation(&self) -> Result<&PathBuf, &BuildStepError> {
        self.documentation
            .as_ref()
            .map(|report| &report.value)
            .map_err(|report| &report.value)
    }

    pub fn rustdoc_json(&self) -> Result<&RustdocJsonOutput, &BuildStepError> {
        self.rustdoc_json
            .as_ref()
            .map(|report| &report.value)
            .map_err(|report| &report.value)
    }

    pub fn regenerate_lockfile(&self) -> Option<&StepResult<()>> {
        self.regenerate_lockfile.as_ref()
    }
}

/// Service-independent result of building one crate release.
pub struct ReleaseBuildResult {
    /// Sandbox statistics captured after all documentation targets finished.
    /// Includes all targets and retry attempts in the shared sandbox.
    pub statistics: SandboxStatistics,
    /// Metadata read from rustwide's prepared source directory.
    pub docsrs_metadata: Metadata,
    /// Cargo's resolved package metadata for the prepared source.
    pub cargo_metadata: CargoMetadata,
    pub default_target: TargetBuildResult,
    pub other_targets: Vec<TargetBuildResult>,
}

impl ReleaseBuildResult {
    /// Whether Cargo completed the default HTML documentation command successfully.
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
    use crate::StepReport;
    use docs_rs_types::BuildError as _;
    use rustwide::cmd::CommandError;
    use test_case::test_case;

    #[test_case(BuildStepError::Command(CommandError::Timeout(1)), "Timeout"; "timeout")]
    #[test_case(BuildStepError::Command(CommandError::SandboxOOM), "SandboxOOM"; "sandbox oom")]
    #[test_case(BuildStepError::Prepare(anyhow::anyhow!("target unavailable")), "InternalPrepare"; "preparation")]
    #[test_case(BuildStepError::Output(anyhow::anyhow!("invalid output")), "InternalOutput"; "output processing")]
    fn classifies_build_step_errors(error: BuildStepError, expected: &str) {
        assert_eq!(error.kind(), expected);
    }

    fn target_result(documentation_path: PathBuf) -> TargetBuildResult {
        TargetBuildResult {
            target: "x86_64-unknown-linux-gnu".into(),
            is_default: true,
            duration: Some(Duration::ZERO),
            compiler_metrics: None,
            documentation: Ok(StepReport {
                value: documentation_path,
                log: None,
                duration: Duration::ZERO,
            }),
            rustdoc_json: Ok(StepReport {
                value: RustdocJsonOutput::new(PathBuf::from("unused.json")),
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
        let missing = target_result(temporary.path().join("missing"));
        assert!(!missing.documentation_exists());
        assert!(missing.build_succeeded());
        assert!(!missing.documentation_succeeded());
        assert!(!missing.has_docs("example_crate"));

        let existing = target_result(temporary.path().to_owned());
        assert!(existing.documentation_exists());
        assert!(existing.build_succeeded());
        assert!(existing.documentation_succeeded());
        assert!(!existing.has_docs("example_crate"));

        std::fs::create_dir(temporary.path().join("example_crate")).unwrap();
        assert!(existing.has_docs("example_crate"));
    }

    #[test]
    fn reads_rustdoc_json_format_version_lazily() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("crate.json");
        std::fs::write(&path, r#"{"format_version":42}"#)?;
        let output = RustdocJsonOutput::new(path);

        assert_eq!(
            output.format_version()?,
            RustdocJsonFormatVersion::Version(42)
        );
        Ok(())
    }

    #[test_case("not JSON"; "malformed json")]
    #[test_case("{}"; "missing format version")]
    fn reports_invalid_rustdoc_json_metadata(contents: &str) -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("crate.json");
        std::fs::write(&path, contents)?;
        let output = RustdocJsonOutput::new(path);

        assert!(output.format_version().is_err());
        Ok(())
    }
}
