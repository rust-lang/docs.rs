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
    /// Rustdoc JSON build result.
    pub rustdoc_json: StepResult<PathBuf>,
    /// Documentation coverage build result.
    pub coverage: StepResult<Option<DocCoverage>>,
}

impl TargetBuildResult {
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

    /// Whether the primary HTML documentation build succeeded and produced output.
    pub fn successful(&self) -> bool {
        self.documentation.successful() && self.documentation_exists()
    }

    /// Whether this target produced documentation for the crate's library target.
    pub fn has_docs(&self, library_name: &str) -> bool {
        self.successful()
            && self
                .documentation
                .output
                .as_ref()
                .is_some_and(|path| path.join(library_name).is_dir())
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

    fn target_result(documentation_path: PathBuf) -> TargetBuildResult {
        TargetBuildResult {
            target: "x86_64-unknown-linux-gnu".into(),
            is_default: true,
            documentation: StepResult {
                output: Some(documentation_path),
                error: None,
                log: String::new(),
            },
            rustdoc_json: StepResult {
                output: None,
                error: None,
                log: String::new(),
            },
            coverage: StepResult {
                output: None,
                error: None,
                log: String::new(),
            },
        }
    }

    #[test]
    fn successful_target_requires_documentation_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let missing = target_result(temporary.path().join("missing"));
        assert!(!missing.documentation_exists());
        assert!(!missing.successful());
        assert!(!missing.has_docs("example_crate"));

        let existing = target_result(temporary.path().to_owned());
        assert!(existing.documentation_exists());
        assert!(existing.successful());
        assert!(!existing.has_docs("example_crate"));

        std::fs::create_dir(temporary.path().join("example_crate")).unwrap();
        assert!(existing.has_docs("example_crate"));
    }
}
