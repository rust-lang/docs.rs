use crate::utils::copy_dir_all;
use anyhow::{Context as _, Result};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use tracing::{debug, instrument, warn};

/// move cache folder to target, falling back to copy + delete on error.
fn move_or_copy<P: AsRef<Path> + std::fmt::Debug, Q: AsRef<Path> + std::fmt::Debug>(
    source: P,
    dest: Q,
) -> io::Result<()> {
    if let Some(parent) = dest.as_ref().parent() {
        fs::create_dir_all(parent)?;
    }
    if let Err(err) = fs::rename(&source, &dest) {
        warn!(
            ?err,
            ?source,
            ?dest,
            "could not move target directory, fall back to copy"
        );
        copy_dir_all(&source, &dest)?;
        fs::remove_dir_all(&source)?;
    }
    Ok(())
}

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
            // to be safe, while most of the time the dir doesn't exist,
            // or is empty.
            fs::remove_dir_all(target_dir).context("could not clean target directory")?;
        }

        let cache_dir = self.cache_dir.join(cache_key);
        if !cache_dir.exists() {
            // when there is no existing cache dir,
            // we can just create an empty target.
            fs::create_dir_all(target_dir).context("could not create target directory")?;
            return Ok(());
        }

        move_or_copy(cache_dir, target_dir).context("could not move cache directory to target")?;
        Ok(())
    }

    #[instrument(skip(self))]
    pub(crate) fn save<P: AsRef<Path> + std::fmt::Debug>(
        &self,
        cache_key: &str,
        target_dir: P,
    ) -> Result<()> {
        let cache_dir = self.cache_dir.join(cache_key);
        if !cache_dir.exists() {
            fs::create_dir_all(&cache_dir)?;
        }

        move_or_copy(&target_dir, &cache_dir)
            .context("could not move target directory to cache")?;
        self.cleanup(&cache_dir)?;
        Ok(())
    }
}
