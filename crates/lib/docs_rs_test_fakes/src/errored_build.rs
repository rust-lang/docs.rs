use docs_rs_types::BuildError;
use std::fmt;

/// An owned error retaining the original display text and classification.
#[derive(Debug)]
pub(crate) struct StoredBuildError {
    message: String,
    kind: &'static str,
}

impl StoredBuildError {
    pub(crate) fn new(error: impl BuildError) -> Self {
        Self {
            message: error.to_string(),
            kind: error.kind(),
        }
    }
}

impl fmt::Display for StoredBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for StoredBuildError {}

impl BuildError for StoredBuildError {
    fn kind(&self) -> &'static str {
        self.kind
    }
}
