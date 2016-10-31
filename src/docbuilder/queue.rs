//! Updates crates.io index and builds new packages


use super::DocBuilder;
use rustc_serialize::json::{Json, Array};
use hyper;
use db::connect_db;
use errors::*;


impl DocBuilder {
    /// Updates crates.io-index repository and adds new crates into build queue
    pub fn get_new_crates(&mut self) -> Result<()> {
        try!(self.load_database_cache());

        let body = {
            use std::io::Read;
            let client = hyper::Client::new();
            let mut res = try!(client.get("https://crates.io/summary").send());
            let mut body = String::new();
            try!(res.read_to_string(&mut body));
            body
        };

        let json = try!(Json::from_str(&body));

        let crates = {
            let mut crates: Vec<(String, String)> = Vec::new();
            for section in ["just_updated", "new_crates"].iter() {
                match json.as_object()
                    .and_then(|o| o.get(&section[..]))
                    .and_then(|j| j.as_array())
                    .map(get_crates_from_array) {
                    Some(mut c) => crates.append(c.as_mut()),
                    None => continue,
                }
            }
            crates
        };

        let conn = try!(connect_db());
        for (name, version) in crates {
            if self.db_cache.contains(&format!("{}-{}", name, version)[..]) {
                continue;
            }
            let _ = conn.execute("INSERT INTO queue (name, version) VALUES ($1, $2)",
                                 &[&name, &version]);
        }

        Ok(())
    }


    /// Builds packages from queue
    pub fn build_packages_queue(&mut self) -> Result<()> {
        let conn = try!(connect_db());

        for row in &try!(conn.query("SELECT id, name, version FROM queue ORDER BY id ASC", &[])) {
            let id: i32 = row.get(0);
            let name: String = row.get(1);
            let version: String = row.get(2);

            match self.build_package(&name[..], &version[..]) {
                Ok(_) => {
                    let _ = conn.execute("DELETE FROM queue WHERE id = $1", &[&id]);
                }
                Err(e) => {
                    error!("Failed to build package {}-{} from queue: {}",
                           name,
                           version,
                           e)
                }
            }
        }

        Ok(())
    }
}


/// Returns Vec<CRATE_NAME, CRATE_VERSION> from a summary array
fn get_crates_from_array<'a>(crates: &'a Array) -> Vec<(String, String)> {
    let mut crates_vec: Vec<(String, String)> = Vec::new();
    for crte in crates {
        let name = match crte.as_object()
            .and_then(|o| o.get("id"))
            .and_then(|i| i.as_string())
            .map(|s| s.to_owned()) {
            Some(s) => s,
            None => continue,
        };
        let version = match crte.as_object()
            .and_then(|o| o.get("max_version"))
            .and_then(|v| v.as_string())
            .map(|s| s.to_owned()) {
            Some(s) => s,
            None => continue,
        };
        crates_vec.push((name, version));
    }
    crates_vec
}




#[cfg(test)]
mod test {
    extern crate env_logger;
    use std::path::PathBuf;
    use {DocBuilder, DocBuilderOptions};

    #[test]
    #[ignore]
    fn test_get_new_crates() {
        let _ = env_logger::init();
        let options = DocBuilderOptions::from_prefix(PathBuf::from("../cratesfyi-prefix"));
        let mut docbuilder = DocBuilder::new(options);
        let res = docbuilder.get_new_crates();
        if res.is_err() {
            error!("{:?}", res);
        }
        assert!(res.is_ok());
    }


    #[test]
    #[ignore]
    fn test_build_packages_queue() {
        let _ = env_logger::init();
        let options = DocBuilderOptions::from_prefix(PathBuf::from("../cratesfyi-prefix"));
        let mut docbuilder = DocBuilder::new(options);
        assert!(docbuilder.build_packages_queue().is_ok());
    }
}
