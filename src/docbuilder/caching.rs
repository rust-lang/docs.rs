use anyhow::{Context as _, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tracing::{debug, instrument, warn};

/// artifact caching with cleanup
#[derive(Debug)]
pub(crate) struct ArtifactCache {
    cache_dir: PathBuf,
}

impl ArtifactCache {
    pub(crate) fn new(cache_dir: PathBuf) -> Result<Self> {
        Ok(Self { cache_dir })
    }

    pub(crate) fn purge(&self) -> Result<()> {
        fs::remove_dir_all(&self.cache_dir)?;
        Ok(())
    }

    /// clean up a target directory.
    ///
    /// Should delete all things that shouldn't leak between
    /// builds, so:
    /// - doc-output
    /// - ...?
    #[instrument(skip(self))]
    fn cleanup(&self, target_dir: &Path) -> Result<()> {
        // proc-macro crates have a `doc` directory
        // directly in the target.
        let doc_dir = target_dir.join("doc");
        if doc_dir.is_dir() {
            debug!(?doc_dir, "cache dir cleanup");
            fs::remove_dir_all(doc_dir)?;
        }

        for item in fs::read_dir(target_dir)? {
            // the first level of directories are the targets in most cases,
            // delete their doc-directories
            let item = item?;
            let doc_dir = item.path().join("doc");
            if doc_dir.is_dir() {
                debug!(?doc_dir, "cache dir cleanup");
                fs::remove_dir_all(doc_dir)?;
            }
        }
        Ok(())
    }

    /// restore a cached target directory.
    ///
    /// Will just move the cache folder into the rustwide
    /// target path. If that fails, will use `copy_dir_all`.
    #[instrument(skip(self))]
    pub(crate) fn restore_to<P: AsRef<Path> + std::fmt::Debug>(
        &self,
        cache_key: &str,
        target_dir: P,
    ) -> Result<()> {
        let target_dir = target_dir.as_ref();
        if target_dir.exists() {
            // Delete the target dir to be safe, even though most of the
            // time the dir doesn't exist or is empty.
            fs::remove_dir_all(target_dir).context("could not clean target directory")?;
        }

        let cache_dir = self.cache_dir.join(cache_key);
        if !cache_dir.exists() {
            // when there is no existing cache dir,
            // we can just create an empty target.
            fs::create_dir_all(target_dir).context("could not create target directory")?;
            return Ok(());
        }

        fs::rename(cache_dir, target_dir).context("could not move cache directory to target")?;
        Ok(())
    }

    #[instrument(skip(self))]
    pub(crate) fn save<P: AsRef<Path> + std::fmt::Debug>(
        &self,
        cache_key: &str,
        target_dir: P,
    ) -> Result<()> {
        let cache_dir = self.cache_dir.join(cache_key);
        if cache_dir.exists() {
            fs::remove_dir_all(&cache_dir)?;
        }

        debug!(?target_dir, ?cache_dir, "saving artifact cache");
        fs::rename(&target_dir, &cache_dir).context("could not move target directory to cache")?;
        self.cleanup(&cache_dir)?;
        Ok(())
    }
}
