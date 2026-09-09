use anyhow::{Context as _, Result};
use docs_rs_cargo_metadata::CargoMetadata;
use docs_rs_rustdoc_json::{RustdocJsonFormatVersion, read_format_version_from_rustdoc_json};
use docs_rs_types::{BuildError, doc_coverage::DocCoverage};
use docsrs_metadata::Metadata;
use rustwide::{SandboxStatistics, cmd::CommandError};
use std::{
    fs::File,
    path::{Path, PathBuf},
    time::Duration,
};
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

/// Failure of an individual build step.
#[derive(Debug, thiserror::Error)]
pub enum BuildStepError {
    /// Cargo or rustdoc failed inside the sandbox.
    #[error(transparent)]
    Command(#[from] CommandError),
    /// The command completed but its output could not be processed.
    #[error(transparent)]
    Output(#[from] anyhow::Error),
}

impl BuildError for BuildStepError {
    fn kind(&self) -> &'static str {
        match self {
            Self::Command(error) => match error {
                CommandError::NoOutputFor(_) => "NoOutputFor",
                CommandError::Timeout(_) => "Timeout",
                CommandError::ExecutionFailed { .. } => "ExecutionFailed",
                CommandError::KillAfterTimeoutFailed(_) => "KillAfterTimeoutFailed",
                CommandError::SandboxOOM => "SandboxOOM",
                CommandError::SandboxImagePullFailed(_) => "SandboxImagePullFailed",
                CommandError::SandboxImageMissing(_) => "SandboxImageMissing",
                CommandError::SandboxContainerCreate(_) => "SandboxContainerCreate",
                CommandError::WorkspaceNotMountedCorrectly => "WorkspaceNotMountedCorrectly",
                CommandError::InvalidDockerInspectOutput(_) => "InvalidDockerInspectOutput",
                CommandError::IO(_) => "IO",
                _ => "UnknownCommandError",
            },
            Self::Output(_) => "Other",
        }
    }
}

/// Output and captured log of one non-fatal release build step.
#[derive(Debug)]
pub struct StepResult<T> {
    /// Wall-clock time spent preparing, executing, and processing this step.
    pub duration: Duration,
    /// Produced value when the step succeeded.
    pub output: Option<T>,
    /// Failure when the step did not succeed.
    pub error: Option<BuildStepError>,
    /// Cargo and rustdoc output captured for this step.
    pub log: String,
}

impl<T> StepResult<T> {
    /// Whether this step completed successfully.
    pub fn successful(&self) -> bool {
        self.error.is_none()
    }
}

/// Results for all build modes of one compilation target.
#[derive(Debug)]
pub struct TargetBuildResult {
    pub(crate) duration: Duration,
    /// Rust target triple.
    pub target: String,
    /// Whether this is the release's default target.
    pub is_default: bool,
    /// HTML documentation output directory.
    pub documentation: StepResult<PathBuf>,
    /// Rustdoc JSON build result.
    pub rustdoc_json: StepResult<RustdocJsonOutput>,
    /// Documentation coverage build result.
    pub coverage: StepResult<Option<DocCoverage>>,
    /// Compiler metrics files copied out of this target's HTML build.
    pub compiler_metrics: Vec<PathBuf>,
}

impl TargetBuildResult {
    /// Elapsed time for this target, including all attempts and lockfile regeneration.
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// Whether rustdoc produced a documentation output directory.
    ///
    /// Cargo can exit successfully without generating documentation for a
    /// target, so command success alone is not sufficient.
    pub fn documentation_exists(&self) -> bool {
        self.documentation
            .output
            .as_ref()
            .is_some_and(|path| path.is_dir())
    }

    /// Whether Cargo completed the primary HTML documentation command successfully.
    pub fn build_succeeded(&self) -> bool {
        self.documentation.successful()
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
                .output
                .as_ref()
                .is_some_and(|path| path.join(library_name).is_dir())
    }
}

/// Service-independent result of building one crate release.
pub struct ReleaseBuildResult {
    /// Sandbox statistics captured after all documentation targets finished.
    /// Includes all targets and retry attempts in the shared sandbox.
    pub statistics: SandboxStatistics,
    /// Metadata read from rustwide's prepared source directory.
    pub metadata: Metadata,
    /// Cargo's resolved package metadata for the prepared source.
    pub cargo_metadata: CargoMetadata,
    /// Default target followed by any requested additional targets.
    pub targets: Vec<TargetBuildResult>,
}

impl ReleaseBuildResult {
    /// The default target result.
    pub fn default_target(&self) -> &TargetBuildResult {
        self.targets
            .first()
            .expect("a release always has a default target")
    }

    /// Whether Cargo completed the default HTML documentation command successfully.
    pub fn build_succeeded(&self) -> bool {
        self.default_target().build_succeeded()
    }

    /// Whether the default HTML documentation build completed and produced output.
    pub fn documentation_succeeded(&self) -> bool {
        self.default_target().documentation_succeeded()
    }

    /// Whether the default target produced documentation for this crate's library target.
    pub fn has_docs(&self) -> bool {
        self.cargo_metadata
            .root()
            .library_name()
            .is_some_and(|name| self.default_target().has_docs(&name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(BuildStepError::Command(CommandError::Timeout(1)), "Timeout"; "timeout")]
    #[test_case(BuildStepError::Command(CommandError::SandboxOOM), "SandboxOOM"; "sandbox oom")]
    #[test_case(BuildStepError::Output(anyhow::anyhow!("invalid output")), "Other"; "output processing")]
    fn classifies_build_step_errors(error: BuildStepError, expected: &str) {
        assert_eq!(error.kind(), expected);
    }

    fn target_result(documentation_path: PathBuf) -> TargetBuildResult {
        TargetBuildResult {
            target: "x86_64-unknown-linux-gnu".into(),
            is_default: true,
            duration: Duration::ZERO,
            documentation: StepResult {
                output: Some(documentation_path),
                error: None,
                log: String::new(),
                duration: Duration::ZERO,
            },
            rustdoc_json: StepResult {
                output: None,
                error: None,
                log: String::new(),
                duration: Duration::ZERO,
            },
            coverage: StepResult {
                output: None,
                error: None,
                log: String::new(),
                duration: Duration::ZERO,
            },
            compiler_metrics: Vec::new(),
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
