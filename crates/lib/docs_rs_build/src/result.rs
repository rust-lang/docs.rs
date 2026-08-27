use docs_rs_cargo_metadata::CargoMetadata;
use docs_rs_types::doc_coverage::DocCoverage;
use docsrs_metadata::Metadata;
use rustwide::cmd::CommandError;
use std::path::PathBuf;

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

/// Output and captured log of one non-fatal release build step.
#[derive(Debug)]
pub struct StepResult<T> {
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
    /// Rust target triple.
    pub target: String,
    /// Whether this is the release's default target.
    pub is_default: bool,
    /// HTML documentation output directory.
    pub documentation: StepResult<PathBuf>,
    /// Rustdoc JSON file, when requested and produced.
    pub rustdoc_json: Option<StepResult<PathBuf>>,
    /// Documentation coverage, when requested.
    pub coverage: Option<StepResult<Option<DocCoverage>>>,
}

impl TargetBuildResult {
    /// Whether the primary HTML documentation build succeeded.
    pub fn successful(&self) -> bool {
        self.documentation.successful()
    }
}

/// Service-independent result of building one crate release.
pub struct ReleaseBuildResult {
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

    /// Whether the default HTML documentation build succeeded.
    pub fn successful(&self) -> bool {
        self.default_target().successful()
    }
}
