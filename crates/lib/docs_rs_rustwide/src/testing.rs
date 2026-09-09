//! Shared support for tests that exercise the docs.rs rustwide workspace.

use crate::SandboxImageSource;
use anyhow::Result;
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};
use tracing::debug;

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
    /// Lock a specific rustwide workspace path for exclusive test use.
    pub fn acquire_at(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        fs::create_dir_all(&path)?;

        let lock_path = path.join(".test-lock");
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(&lock_path)?;

        debug!(workspace = %path.display(), lockfile = %lock_path.display(), "waiting for test workspace lock");
        lock.lock()?;
        debug!(workspace = %path.display(), lockfile = %lock_path.display(), "acquired test workspace lock");

        Ok(Self { path, _lock: lock })
    }

    /// Path of the locked rustwide workspace.
    pub fn path(&self) -> &Path {
        &self.path
    }
}
