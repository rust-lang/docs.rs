mod crates;
mod limits;
pub(crate) mod options;
mod queue;
mod rustwide_builder;

pub(crate) use self::limits::Limits;
pub use self::rustwide_builder::RustwideBuilder;
pub(crate) use self::rustwide_builder::{BuildResult, DocCoverage};

use crate::db::Pool;
use crate::error::Result;
use crate::index::Index;
use crate::BuildQueue;
use crate::DocBuilderOptions;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

/// chroot based documentation builder
pub struct DocBuilder {
    options: DocBuilderOptions,
    index: Index,
    db: Pool,
    build_queue: Arc<BuildQueue>,
}

impl DocBuilder {
    pub fn new(options: DocBuilderOptions, db: Pool, build_queue: Arc<BuildQueue>) -> DocBuilder {
        let index = Index::new(&options.registry_index_path).expect("valid index");
        DocBuilder {
            build_queue,
            options,
            index,
            db,
        }
    }

    fn lock_path(&self) -> PathBuf {
        self.options.prefix.join("cratesfyi.lock")
    }

    /// Creates a lock file. Daemon will check this lock file and stop operating if its exists.
    pub fn lock(&self) -> Result<()> {
        let path = self.lock_path();
        if !path.exists() {
            fs::OpenOptions::new().write(true).create(true).open(path)?;
        }

        Ok(())
    }

    /// Removes lock file.
    pub fn unlock(&self) -> Result<()> {
        let path = self.lock_path();
        if path.exists() {
            fs::remove_file(path)?;
        }

        Ok(())
    }

    /// Checks for the lock file and returns whether it currently exists.
    pub fn is_locked(&self) -> bool {
        self.lock_path().exists()
    }

    /// Returns a reference of options
    pub fn options(&self) -> &DocBuilderOptions {
        &self.options
    }
}
