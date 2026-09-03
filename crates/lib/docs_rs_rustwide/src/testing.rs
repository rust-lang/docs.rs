//! Shared support for tests that exercise the docs.rs rustwide workspace.

use crate::SandboxImageSource;
use anyhow::Result;
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

/// The small sandbox image used by docs.rs build tests.
pub const TEST_SANDBOX_IMAGE: &str = "ghcr.io/rust-lang/crates-build-env/linux-micro";

const TEST_WORKSPACE_ENV: &str = "DOCSRS_RUSTWIDE_WORKSPACE";

/// A persistent rustwide workspace locked for exclusive use by one test.
///
/// Reusing the workspace avoids reinstalling rustup and the configured
/// toolchain for every ignored integration test. The lock prevents tests from
/// concurrently purging or updating the shared workspace.
pub struct TestWorkspace {
    path: PathBuf,
    _lock: File,
}

impl TestWorkspace {
    /// Lock the persistent workspace configured for docs.rs tests.
    pub fn acquire() -> Result<Self> {
        Self::acquire_at(test_workspace_path())
    }

    /// Lock a specific rustwide workspace path for exclusive test use.
    pub fn acquire_at(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut lock_path = path.as_os_str().to_os_string();
        lock_path.push(".test-lock");
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(PathBuf::from(lock_path))?;
        tracing::debug!(workspace = %path.display(), "waiting for test workspace lock");
        lock.lock()?;
        tracing::debug!(workspace = %path.display(), "acquired test workspace lock");

        Ok(Self { path, _lock: lock })
    }

    /// Path of the locked rustwide workspace.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The micro sandbox image configured with local-or-remote resolution.
pub fn test_sandbox_image() -> SandboxImageSource {
    SandboxImageSource::LocalOrRemote(TEST_SANDBOX_IMAGE.into())
}

/// Path shared by docs.rs tests that exercise rustwide.
pub fn test_workspace_path() -> PathBuf {
    std::env::var_os(TEST_WORKSPACE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(3)
                .expect("docs_rs_rustwide must be inside the workspace")
                .join(".workspace")
        })
}
