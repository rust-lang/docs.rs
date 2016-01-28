//! This is a simle crate module

use std::io::prelude::*;
use std::io::BufReader;
use std::io::Error;
use std::fs::File;
use std::path::PathBuf;

use rustc_serialize::json::Json;
use rustc_serialize::json::ParserError;


/// Really simple crate model
#[derive(Debug)]
pub struct Crate {
    /// Name of crate
    pub name: String,
    /// Versions of crate
    pub versions: Vec<String>,
}


#[derive(Debug)]
pub enum CrateOpenError {
    FileNotFound,
    ParseError(ParserError),
    IoError(Error),
    NotObject,
    NameNotFound,
    VersNotFound,
}


impl Crate {
    /// Returns a new Crate
    ///
    /// # Examples
    ///
    /// ```
    /// let crte = Crate::new("cratesfyi".to_string(),
    ///                       vec!["0.1.0".to_string()]);
    /// assert_eq!(crte.name, "cratesfyi");
    /// ```
    pub fn new(name: String, versions: Vec<String>) -> Crate {
        Crate {
            name: name,
            versions: versions,
        }
    }

    pub fn from_cargo_index_file(path: PathBuf) -> Result<Crate, CrateOpenError> {

        let mut file = match File::open(path) {
            Ok(f) => BufReader::new(f),
            Err(_) => return Err(CrateOpenError::FileNotFound),
        };

        let mut line = String::new();

        let mut name = String::new();
        let mut versions = Vec::new();

        while file.read_line(&mut line).unwrap() > 0 {

            let (cname, vers) = match Crate::parse_cargo_index_line(&line) {
                Ok((c, v)) => (c, v),
                Err(e) => return Err(e)
            };

            name = cname;
            versions.push(vers);

            line.clear();
        }

        versions.reverse();

        Ok(Crate {
            name: name,
            versions: versions
        })
    }


    /// Create Crate from crate name and try to find it in crates.io-index
    /// to load versions
    pub fn from_cargo_index_path(name: &str, path: &PathBuf) -> Result<Crate, CrateOpenError> {

        if !path.is_dir() {
            return Err(CrateOpenError::FileNotFound);
        }

        for file in try!(path.read_dir().map_err(CrateOpenError::IoError)) {

            let file = try!(file.map_err(CrateOpenError::IoError));

            let path = file.path();

            // skip files under .git and config.json
            if path.to_str().unwrap().contains(".git") ||
                path.file_name().unwrap() == "config.json" {
                    continue;
                }

            if path.is_dir() {
                if let Ok(c) = Crate::from_cargo_index_path(&name, &path) {
                    return Ok(c);
                }
            } else if file.file_name().into_string().unwrap() == name {
                return Crate::from_cargo_index_file(path);
            }

        }

        Err(CrateOpenError::FileNotFound)
    }


    fn parse_cargo_index_line(line: &String) -> Result<(String, String), CrateOpenError> {
        let data = try!(Json::from_str(line.trim()).map_err(CrateOpenError::ParseError));
        let obj = try!(data.as_object().ok_or(CrateOpenError::NotObject));

        let crate_name = try!(obj.get("name")
                              .and_then(|n| n.as_string())
                              .ok_or(CrateOpenError::NameNotFound));

        let vers = try!(obj.get("vers")
                        .and_then(|n| n.as_string())
                        .ok_or(CrateOpenError::VersNotFound));

        Ok((String::from(crate_name), String::from(vers)))
    }


    pub fn get_version_index(&self, requested_version: &str) -> Option<usize> {
        for i in 0..self.versions.len() {
            if self.versions[i] == requested_version {
                return Some(i);
            }
        }
        None
    }


    pub fn version_starts_with(&self, version: &str) -> Option<usize> {
        // if version is "*" return latest version index which is 0
        if version == "*" {
            return Some(0);
        }
        for i in 0..self.versions.len() {
            if self.versions[i].starts_with(&version) {
                return Some(i)
            }
        }
        None
    }


    pub fn canonical_name(&self, version_index: usize) -> String {
        format!("{}-{}", self.name, self.versions[version_index])
    }

}


#[cfg(test)]
mod test {
    use super::*;
    use std::env;
    use std::path::PathBuf;

    #[test]
    fn test_get_vesion_index() {
        let crte = Crate::new("cratesfyi".to_string(),
                              vec!["0.1.0".to_string(), "0.1.1".to_string()]);
        assert_eq!(crte.get_version_index("0.1.0"), Some(0));
        assert_eq!(crte.get_version_index("0.1.1"), Some(1));
        assert_eq!(crte.get_version_index("0.1.2"), None);
    }


    // Rest of the tests only works if crates.io-index is exists in:
    // ../cratesfyi-prefix/crates.io-index

    #[test]
    #[ignore]
    fn test_from_cargo_index_path() {
        let mut path = PathBuf::from(env::current_dir().unwrap());
        path.push("../cratesfyi-prefix/crates.io-index");

        if !path.exists() {
            return;
        }

        let crte = Crate::from_cargo_index_path("rand", &path).unwrap();
        assert_eq!(crte.name, "rand");
        assert!(crte.versions.len() > 0);
    }


    #[test]
    #[ignore]
    fn test_version_starts_with() {
        let mut path = PathBuf::from(env::current_dir().unwrap());
        path.push("../cratesfyi-prefix/crates.io-index");

        if !path.exists() {
            return;
        }

        let crte = Crate::from_cargo_index_path("rand", &path).unwrap();
        assert!(crte.version_starts_with("0.1".to_string()).is_some());
        assert!(crte.version_starts_with("*".to_string()).is_some());
        assert!(crte.version_starts_with("999.099.99".to_string()).is_none());
    }

}
