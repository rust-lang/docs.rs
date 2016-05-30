
pub mod error;
pub mod options;
mod chroot_builder;
mod crates;

pub use self::chroot_builder::ChrootBuilderResult;


use std::fs;
use std::io::prelude::*;
use std::io::BufReader;
use std::path::PathBuf;
use std::collections::BTreeSet;
use ::{DocBuilderError, DocBuilderOptions};


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
    pub fn load_cache(&mut self) -> Result<(), DocBuilderError> {
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


    fn load_database_cache(&mut self) -> Result<(), DocBuilderError> {
        debug!("Loading database cache");
        use db::connect_db;
        let conn = try!(connect_db());

        for row in &conn.query("SELECT name, version FROM crates, releases \
                               WHERE crates.id = releases.crate_id", &[]).unwrap() {
            let name: String = row.get(0);
            let version: String = row.get(1);
            self.db_cache.insert(format!("{}-{}", name, version));
        }

        Ok(())
    }


    /// Saves build cache
    pub fn save_cache(&self) -> Result<(), DocBuilderError> {
        debug!("Saving cache");
        let path = PathBuf::from(&self.options.prefix).join("cache");
        let mut file = try!(fs::OpenOptions::new().write(true).create(true)
                            .open(path));
        for krate in &self.cache {
            try!(writeln!(file, "{}", krate));
        }
        Ok(())
    }
}



#[cfg(test)]
mod test {
    extern crate env_logger;
    use ::DocBuilderOptions;
    use super::*;
    use std::path::PathBuf;

    #[test]
    #[ignore]
    fn test_docbuilder_crates() {
        let _ = env_logger::init();
        let options = DocBuilderOptions::from_prefix(PathBuf::from("../cratesfyi-prefix"));
        let docbuilder = DocBuilder::new(options);
        let res = docbuilder.crates(|_, _| {});
        assert!(res.is_ok());
    }
}
