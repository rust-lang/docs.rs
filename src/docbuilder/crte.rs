//! This is a simle crate module

use std::io::prelude::*;
use std::io::BufReader;
use std::io::Error;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::collections;
use std::env;

use toml;
use rustc_serialize::json::Json;
use rustc_serialize::json::ParserError;

use super::{DocBuilder, DocBuilderError, copy_files, command_result};


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

        let mut file = match fs::File::open(path) {
            Ok(f) => BufReader::new(f),
            Err(_) => return Err(CrateOpenError::FileNotFound),
        };

        let mut line = String::new();

        let mut name = String::new();
        let mut versions = Vec::new();

        while try!(file.read_line(&mut line).map_err(CrateOpenError::IoError)) > 0 {

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


    pub fn extract_crate(&self, version_index: usize) -> Result<String, String> {
        let crate_name = format!("{}.crate", self.canonical_name(version_index));
        command_result(Command::new("tar")
                       .arg("-xzvf")
                       .arg(crate_name)
                       .output()
                       .unwrap())
    }


    /// Downloads crate
    pub fn download_crate(&self, version_index: usize) -> Result<String, String> {
        // By default crates.io is using:
        // https://crates.io/api/v1/crates/$crate/$version/download
        // But I believe this url is increasing download count and this program is
        // downloading alot during development. I am using redirected url.
        let url = format!("https://crates-io.s3-us-west-1.amazonaws.com/crates/{}/{}-{}.crate",
                          self.name,
                          self.name,
                          self.versions[version_index]);
        // Use wget for now
        command_result(Command::new("wget")
                       .arg("-c")
                       .arg("--content-disposition")
                       .arg(url)
                       .output()
                       .unwrap())
    }



    /// Download local dependencies from crate root and place them into right place
    ///
    /// Some packages have local dependencies defined in Cargo.toml
    ///
    /// This function is intentionall written verbose
    fn download_dependencies(&self, root_dir: &PathBuf, docbuilder: &DocBuilder) -> Result<(), DocBuilderError> {

        let mut cargo_toml_path = PathBuf::from(&root_dir);
        cargo_toml_path.push("Cargo.toml");

        let mut cargo_toml_fh = try!(fs::File::open(cargo_toml_path)
                                     .map_err(DocBuilderError::LocalDependencyIoError));
        let mut cargo_toml_content = String::new();
        try!(cargo_toml_fh.read_to_string(&mut cargo_toml_content)
             .map_err(DocBuilderError::LocalDependencyIoError));

        toml::Parser::new(&cargo_toml_content[..]).parse().as_ref()
            .and_then(|cargo_toml| cargo_toml.get("dependencies"))
            .and_then(|dependencies| dependencies.as_table())
            .and_then(|dependencies_table| self.get_local_dependencies(dependencies_table, docbuilder))
            .map(|local_dependencies| self.handle_local_dependencies(local_dependencies, &root_dir))
            .unwrap_or(Ok(()))
    }


    /// Get's local_dependencies from dependencies_table
    fn get_local_dependencies(&self,
                              dependencies_table: &collections::BTreeMap<String, toml::Value>,
                              docbuilder: &DocBuilder) -> Option<Vec<(Crate, usize, String)>>  {

        let mut local_dependencies: Vec<(Crate, usize, String)> = Vec::new();

        for key in dependencies_table.keys() {

            dependencies_table.get(key)
                .and_then(|key_val| key_val.as_table())
                .map(|key_table| {
                    key_table.get("path").and_then(|p| p.as_str()).map(|path| {
                        key_table.get("version").and_then(|p| p.as_str())
                            .map(|version| {
                                // TODO: This kinda became a mess
                                //       I wonder if can use more and_then's...
                                if let Ok(dep_crate) = Crate::from_cargo_index_path(&key,
                                                            &docbuilder.crates_io_index_path) {
                                    if let Some(version_index) =
                                        dep_crate.version_starts_with(version) {
                                        local_dependencies.push((dep_crate,
                                                                 version_index,
                                                                 path.to_string()));
                                    }
                                }
                            });
                    });
                });

        }
        Some(local_dependencies)
    }


    /// Handles local dependencies
    fn handle_local_dependencies(&self,
                                 local_dependencies: Vec<(Crate, usize, String)>,
                                 root_dir: &PathBuf) -> Result<(), DocBuilderError> {
        for local_dependency in local_dependencies {
            let crte = local_dependency.0;
            let version_index = local_dependency.1;

            let mut path = PathBuf::from(&root_dir);
            path.push(local_dependency.2);

            // make sure path exists
            if !path.exists() {
                try!(fs::create_dir_all(&path).map_err(DocBuilderError::LocalDependencyIoError));
            }

            try!(crte.download_crate(version_index)
                 .map_err(DocBuilderError::LocalDependencyDownloadError));
            try!(crte.extract_crate(version_index)
                 .map_err(DocBuilderError::LocalDependencyExtractCrateError));

            let crte_download_dir = PathBuf::from(format!("{}-{}",
                                                          crte.name,
                                                          crte.versions[version_index]));

            if !crte_download_dir.exists() {
                return Err(DocBuilderError::LocalDependencyDownloadDirNotExist);
            }


            // self.extract_crate will extract crate into build_dir
            // Copy files to proper location
            try!(copy_files(&crte_download_dir, &path, false));

            // Remove download_dir
            try!(fs::remove_dir_all(&crte_download_dir)
                 .map_err(DocBuilderError::LocalDependencyIoError));

            try!(crte.remove_crate_file(version_index));
        }

        Ok(())
    }


    fn remove_build_dir_for_crate(&self,
                                  version_index: usize) -> Result<(), DocBuilderError> {
        let path = PathBuf::from(self.canonical_name(version_index));

        if path.exists() && path.is_dir() {
            try!(fs::remove_dir_all(&path).map_err(DocBuilderError::RemoveBuildDir));
        }

        Ok(())
    }


    pub fn build_crate_doc(&self,
                           version_index: usize,
                           docbuilder: &DocBuilder) -> Result<(), DocBuilderError> {


        let package_root = PathBuf::from(self.canonical_name(version_index));

        info!("Building documentation for {}-{}", self.name, self.versions[version_index]);

        // removing old build directory
        try!(self.remove_build_dir_for_crate(version_index));

        // Download crate
        // FIXME: Need to capture failed command outputs
        info!("Downloading crate\n{}",
              try!(self.download_crate(version_index)
                   .map_err(DocBuilderError::DownloadCrateError)));

        // Extract crate
        info!("Extracting crate\n{}",
              try!(self.extract_crate(version_index)
                   .map_err(DocBuilderError::ExtractCrateError)));

        info!("Checking local dependencies");
        try!(self.download_dependencies(&package_root, &docbuilder));

        // build docs
        info!("Building documentation");
        let (status, message) = match self.build_doc(version_index) {
            Ok(m) => (true, m),
            Err(m) => (false, m),
        };
        info!("{}", message);

        if status {
            Ok(())
        } else {
            Err(DocBuilderError::FailedToBuildCrate)
        }
    }


    fn build_doc(&self, version_index: usize) -> Result<String, String> {
        let cwd = env::current_dir().unwrap();
        let mut target = PathBuf::from(&cwd);
        target.push(self.canonical_name(version_index));
        env::set_current_dir(target).unwrap();
        let res = command_result(Command::new("cargo")
                                 .arg("doc")
                                 .arg("--no-deps")
                                 .arg("--verbose")
                                 .output()
                                 .unwrap());
        env::set_current_dir(cwd).unwrap();
        res
    }


    pub fn remove_crate_file(&self,
                             version_index: usize) -> Result<(), DocBuilderError>{
        let path = PathBuf::from(format!("{}.crate", self.canonical_name(version_index)));

        if path.exists() && path.is_file() {
            try!(fs::remove_file(path).map_err(DocBuilderError::RemoveCrateFile));
        }

        Ok(())
    }

}


#[cfg(test)]
mod test {
    use super::*;
    use std::env;
    use std::path::PathBuf;
    use std::fs;
    use std::thread::sleep;

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
        assert!(crte.version_starts_with("0.1").is_some());
        assert!(crte.version_starts_with("*").is_some());
        assert!(crte.version_starts_with("999.099.99").is_none());
    }


    #[test]
    fn test_download_crate() {
        let crte = Crate::new("rand".to_string(),
                              vec!["0.3.13".to_string()]);
        assert!(crte.download_crate(0).is_ok());
        // remove downloaded file
        fs::remove_file(format!("{}.crate", crte.canonical_name(0))).unwrap();
    }


    #[test]
    fn test_extract_crate() {
        let crte = Crate::new("rand".to_string(),
                              vec!["0.3.12".to_string()]);
        assert!(crte.download_crate(0).is_ok());
        assert!(crte.extract_crate(0).is_ok());

        let path = PathBuf::from(crte.canonical_name(0));
        assert!(path.exists());
        fs::remove_dir_all(crte.canonical_name(0)).unwrap();
    }

    #[test]
    fn test_remove_crate_file() {
        let crte = Crate::new("rand".to_string(),
                              vec!["0.3.11".to_string()]);
        assert!(crte.download_crate(0).is_ok());
        assert!(crte.remove_crate_file(0).is_ok());
    }

}
