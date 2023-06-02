use anyhow::{bail, Context as _, Result};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    time::SystemTime,
};
use sysinfo::{DiskExt, RefreshKind, System, SystemExt};
use tracing::{debug, instrument, warn};

static LAST_ACCESSED_FILE_NAME: &str = "docsrs_last_accessed";

/// gives you the percentage of free disk space on the
/// filesystem where the given `path` lives on.
/// Return value is between 0 and 1.
fn free_disk_space_ratio<P: AsRef<Path>>(path: P) -> Result<f32> {
    let sys = System::new_with_specifics(RefreshKind::new().with_disks());

    let disk_by_mount_point: HashMap<_, _> =
        sys.disks().iter().map(|d| (d.mount_point(), d)).collect();

    let path = path.as_ref();

    if let Some(disk) = path.ancestors().find_map(|p| disk_by_mount_point.get(p)) {
        Ok((disk.available_space() as f64 / disk.total_space() as f64) as f32)
    } else {
        bail!("could not find mount point for path {}", path.display());
    }
}

/// artifact caching with cleanup
#[derive(Debug)]
pub(crate) struct ArtifactCache {
    cache_dir: PathBuf,
}

impl ArtifactCache {
    pub(crate) fn new(cache_dir: PathBuf) -> Result<Self> {
        let cache = Self { cache_dir };
        cache.ensure_cache_exists()?;
        Ok(cache)
    }

    pub(crate) fn purge(&self) -> Result<()> {
        fs::remove_dir_all(&self.cache_dir)?;
        self.ensure_cache_exists()?;
        Ok(())
    }

    fn ensure_cache_exists(&self) -> Result<()> {
        if !self.cache_dir.exists() {
            fs::create_dir_all(&self.cache_dir)?;
        }
        Ok(())
    }

    /// clean up a target directory.
    ///
    /// Will:
    /// * delete the doc output in the root & target directories
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

    fn cache_dir_for_key(&self, cache_key: &str) -> PathBuf {
        self.cache_dir.join(cache_key)
    }

    /// update the "last used" marker for the cache key
    fn touch(&self, cache_key: &str) -> Result<()> {
        let file = self
            .cache_dir_for_key(cache_key)
            .join(LAST_ACCESSED_FILE_NAME);

        fs::create_dir_all(file.parent().expect("we always have a parent"))?;
        if file.exists() {
            fs::remove_file(&file)?;
        }
        OpenOptions::new().create(true).write(true).open(&file)?;
        Ok(())
    }

    /// return the list of cache-directories, sorted by last usage.
    ///
    /// The oldest / least used cache will be first.
    /// To be used for cleanup.
    ///
    /// A missing age-marker file is interpreted as "old age".
    fn all_cache_folders_by_age(&self) -> Result<Vec<PathBuf>> {
        let mut entries: Vec<(PathBuf, Option<SystemTime>)> = fs::read_dir(&self.cache_dir)?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                path.is_dir().then(|| {
                    let last_accessed = path
                        .join(LAST_ACCESSED_FILE_NAME)
                        .metadata()
                        .and_then(|metadata| metadata.modified())
                        .ok();
                    (path, last_accessed)
                })
            })
            .collect();

        // `None` will appear first after sorting
        entries.sort_by_key(|(_, time)| *time);

        Ok(entries.into_iter().map(|(path, _)| path).collect())
    }

    /// free up disk space by deleting the oldest cache folders.
    ///
    /// Deletes cache folders until the `free_percent_goal` is reached.
    pub(crate) fn clear_disk_space(&self, free_percent_goal: f32) -> Result<()> {
        let space_ok =
            || -> Result<bool> { Ok(free_disk_space_ratio(&self.cache_dir)? >= free_percent_goal) };

        if space_ok()? {
            return Ok(());
        }

        for folder in self.all_cache_folders_by_age()? {
            warn!(
                ?folder,
                "freeing up disk space by deleting oldest cache folder"
            );
            fs::remove_dir_all(&folder)?;

            if space_ok()? {
                break;
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

        let cache_dir = self.cache_dir_for_key(cache_key);
        if !cache_dir.exists() {
            // when there is no existing cache dir,
            // we can just create an empty target.
            fs::create_dir_all(target_dir).context("could not create target directory")?;
            return Ok(());
        }

        fs::create_dir_all(target_dir.parent().unwrap())?;
        fs::rename(cache_dir, target_dir).context("could not move cache directory to target")?;
        self.cleanup(target_dir)?;
        Ok(())
    }

    #[instrument(skip(self))]
    pub(crate) fn save<P: AsRef<Path> + std::fmt::Debug>(
        &self,
        cache_key: &str,
        target_dir: P,
    ) -> Result<()> {
        let cache_dir = self.cache_dir_for_key(cache_key);
        if cache_dir.exists() {
            fs::remove_dir_all(&cache_dir)?;
        }
        self.ensure_cache_exists()?;

        debug!(?target_dir, ?cache_dir, "saving artifact cache");
        fs::rename(&target_dir, &cache_dir).context("could not move target directory to cache")?;
        self.cleanup(&cache_dir)?;
        self.touch(cache_key)?;
        Ok(())
    }
}
