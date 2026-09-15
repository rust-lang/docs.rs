//! Shared configuration for tests that exercise the docs.rs rustwide workspace.

use std::{
    env,
    path::{Path, PathBuf},
};

/// Persistent workspace used by integration tests. BuildEnvironment owns its lock.
///
/// For better build speed, should be shared across tests using the rustwide builder:
/// * `docs_rs_rustwide` lib integration tests
/// * `docs_rs_builder` bin build-tests
pub fn test_workspace_path() -> PathBuf {
    env::var_os("DOCSRS_RUSTWIDE_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join(".workspace"))
}
