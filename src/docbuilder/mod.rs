
pub mod options;
mod chroot_builder;
mod crates;
mod queue;

pub use self::chroot_builder::ChrootBuilderResult;


use std::fs;
use std::io::prelude::*;
use std::io::BufReader;
use std::path::PathBuf;
use std::collections::BTreeSet;
use DocBuilderOptions;
use error::Result;


/// chroot based documentation builder
pub struct DocBuilder {
    options: DocBuilderOptions,
    cache: BTreeSet<String>,
    db_cache: BTreeSet<String>,
}


impl DocBuilder {
    pub fn new(options: DocBuilderOptions) -> DocBuilder {
        DocBuilder {
            options: options,
            cache: BTreeSet::new(),
            db_cache: BTreeSet::new(),
        }
    }


    /// Loads build cache
    pub fn load_cache(&mut self) -> Result<()> {
        debug!("Loading cache");
        let path = PathBuf::from(&self.options.prefix).join("cache");
        let reader = fs::File::open(path).map(|f| BufReader::new(f));

        if reader.is_err() {
            return Ok(());
        }

        for line in reader.unwrap().lines() {
            self.cache.insert(try!(line));
        }

        try!(self.load_database_cache());

        Ok(())
    }


    fn load_database_cache(&mut self) -> Result<()> {
        debug!("Loading database cache");
        use db::connect_db;
        let conn = try!(connect_db());

        for row in &conn.query("SELECT name, version FROM crates, releases \
                               WHERE crates.id = releases.crate_id",
                   &[])
            .unwrap() {
            let name: String = row.get(0);
            let version: String = row.get(1);
            self.db_cache.insert(format!("{}-{}", name, version));
        }

        Ok(())
    }


    /// Saves build cache
    pub fn save_cache(&self) -> Result<()> {
        debug!("Saving cache");
        let path = PathBuf::from(&self.options.prefix).join("cache");
        let mut file = try!(fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(path));
        for krate in &self.cache {
            try!(writeln!(file, "{}", krate));
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
            try!(fs::OpenOptions::new().write(true).create(true).open(path));
        }
        Ok(())
    }

    /// Removes lock file.
    pub fn unlock(&self) -> Result<()> {
        let path = self.lock_path();
        if path.exists() {
            try!(fs::remove_file(path));
        }
        Ok(())
    }
}
