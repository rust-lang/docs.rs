//! Shared configuration for tests that exercise the docs.rs rustwide workspace.

use crate::SandboxImageSource;
use std::path::{Path, PathBuf};

pub const TEST_SANDBOX_IMAGE: &str = "ghcr.io/rust-lang/crates-build-env/linux-micro";

/// Persistent workspace used by integration tests. BuildEnvironment owns its lock.
pub fn test_workspace_path() -> PathBuf {
    std::env::var_os("DOCSRS_RUSTWIDE_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(3)
                .expect("docs_rs_rustwide must be inside the workspace")
                .join(".workspace")
        })
}

pub fn test_sandbox_image() -> SandboxImageSource {
    SandboxImageSource::LocalOrRemote(TEST_SANDBOX_IMAGE.into())
}
