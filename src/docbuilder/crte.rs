//! This is a simle crate module

use std::io::BufRead;
use std::io::BufReader;
use std::fs::File;
use std::path::PathBuf;

use rustc_serialize::json::Json;
use rustc_serialize::json::ParserError;


/// We are only using crate name and versions in this program
#[derive(Debug)]
pub struct Crate {
    pub name: String,
    pub versions: Vec<String>,
}


#[derive(Debug)]
pub enum CrateOpenError {
    FileNotFound,
    ParseError(ParserError),
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


    fn parse_cargo_index_line(line: &String) -> Result<(String, String), CrateOpenError> {

        let data = match Json::from_str(line.trim()) {
            Ok(json) => json,
            Err(e) => return Err(CrateOpenError::ParseError(e)),
        };

        let obj = match data.as_object() {
            Some(o) => o,
            None => return Err(CrateOpenError::NotObject),
        };

        // try to get name and vers(ion)
        let crate_name = match obj.get("name") {
            Some(n) => {
                match n.as_string() {
                    Some(s) => s,
                    None => return Err(CrateOpenError::NameNotFound),
                }
            }
            None => return Err(CrateOpenError::NameNotFound),
        };

        let vers = match obj.get("vers") {
            Some(n) => {
                match n.as_string() {
                    Some(s) => s,
                    None => return Err(CrateOpenError::VersNotFound),
                }
            }
            None => return Err(CrateOpenError::VersNotFound),
        };

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


    pub fn canonical_name(&self, version_index: usize) -> String {
        format!("{}-{}", self.name, self.versions[version_index])
    }

}


#[test]
fn test_get_vesion_index() {
    let crte = Crate::new("cratesfyi".to_string(),
                          vec!["0.1.0".to_string(), "0.1.1".to_string()]);
    assert_eq!(crte.get_version_index("0.1.0"), Some(0));
    assert_eq!(crte.get_version_index("0.1.1"), Some(1));
    assert_eq!(crte.get_version_index("0.1.2"), None);
}
