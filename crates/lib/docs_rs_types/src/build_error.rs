use std::{error, fmt};

/// a trait for build errors.
///
/// Idea is:
/// Every build error has
/// * a text representation, and
/// * a "kind" (error code)
pub trait BuildError: error::Error + fmt::Display + fmt::Debug + Sized {
    fn kind(&self) -> &'static str;
}

/// a simple build error struct, mostly for testing & utilities.
///
/// The "real" build error is `RustwideBuildError` in our builder subcrate.
#[derive(thiserror::Error, Debug)]
#[error("build error: {0}")]
pub struct SimpleBuildError(pub String);
impl BuildError for SimpleBuildError {
    fn kind(&self) -> &'static str {
        "SimpleBuildError"
    }
}

impl From<&str> for SimpleBuildError {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}
