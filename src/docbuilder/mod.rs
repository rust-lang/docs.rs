mod crates;
mod limits;
mod metadata;
pub(crate) mod options;
mod queue;
mod rustwide_builder;

pub(crate) use self::limits::Limits;
pub(self) use self::metadata::Metadata;
pub(crate) use self::rustwide_builder::BuildResult;
pub use self::rustwide_builder::RustwideBuilder;

use crate::db::Pool;
use crate::error::Result;
use crate::index::Index;
use crate::BuildQueue;
use crate::DocBuilderOptions;
use futures_util::stream::StreamExt;
use log::debug;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
};

/// chroot based documentation builder
pub struct DocBuilder {
    options: DocBuilderOptions,
    index: Index,
    db: Pool,
    build_queue: Arc<BuildQueue>,
    cache: BTreeSet<String>,
    db_cache: BTreeSet<String>,
}

impl DocBuilder {
    pub fn new(options: DocBuilderOptions, db: Pool, build_queue: Arc<BuildQueue>) -> DocBuilder {
        let index = Index::new(&options.registry_index_path).expect("valid index");
        DocBuilder {
            build_queue,
            options,
            index,
            db,
            cache: BTreeSet::new(),
            db_cache: BTreeSet::new(),
        }
    }

    /// Loads build cache
    pub async fn load_cache(&mut self) -> Result<()> {
        debug!("Loading cache");

        let path = PathBuf::from(&self.options.prefix).join("cache");
        let reader = File::open(path).await.map(BufReader::new);

        if let Ok(reader) = reader {
            let mut lines = reader.lines();

            while let Some(line) = lines.next().await.transpose()? {
                self.cache.insert(line);
            }
        }

        self.load_database_cache()?;

        Ok(())
    }

    fn load_database_cache(&mut self) -> Result<()> {
        debug!("Loading database cache");

        let mut conn = self.db.get()?;
        for row in &mut conn.query(
            "SELECT name, version FROM crates, releases \
             WHERE crates.id = releases.crate_id",
            &[],
        )? {
            let name: String = row.get(0);
            let version: String = row.get(1);

            self.db_cache.insert(format!("{}-{}", name, version));
        }

        Ok(())
    }

    /// Saves build cache
    pub async fn save_cache(&self) -> Result<()> {
        debug!("Saving cache");

        let path = PathBuf::from(&self.options.prefix).join("cache");
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .open(path)
            .await?;

        for krate in &self.cache {
            file.write_all(krate.as_bytes()).await?;
        }

        Ok(())
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

    fn add_to_cache(&mut self, name: &str, version: &str) {
        self.cache.insert(format!("{}-{}", name, version));
    }

    fn should_build(&self, name: &str, version: &str) -> bool {
        let name = format!("{}-{}", name, version);
        let local = self.options.skip_if_log_exists && self.cache.contains(&name);
        let db = self.options.skip_if_exists && self.db_cache.contains(&name);

        !(local || db)
    }
}
