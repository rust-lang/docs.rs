
//! DocBuilder

pub mod crte;

use std::io::prelude::*;
use std::io;
use std::fmt;
use std::env;
use std::path::PathBuf;
use std::fs;
use std::process::{Command, Output};
use std::collections;
use std::convert;

use toml;


/// Alright
pub struct DocBuilder {
    keep_build_directory: bool,
    destination: PathBuf,
    chroot_path: PathBuf,
    chroot_user: String,
    build_dir: PathBuf,
    crates_io_index_path: PathBuf,
    logs_path: PathBuf,
    skip_if_exist: bool,
    skip_if_log_exists: bool,
    debug: bool,
}



impl Default for DocBuilder {
    fn default() -> DocBuilder {

        let cwd = env::current_dir().unwrap();

        let mut destination = PathBuf::from(&cwd);
        destination.push("public_html/crates");

        let mut chroot_path = PathBuf::from(&cwd);
        chroot_path.push("chroot");

        let mut build_dir = PathBuf::from(&cwd);
        build_dir.push(&chroot_path);
        build_dir.push("home/onur");

        let mut crates_io_index_path = PathBuf::from(&cwd);
        crates_io_index_path.push("crates.io-index");

        let mut logs_path = PathBuf::from(&cwd);
        logs_path.push("logs");

        DocBuilder {
            destination: destination,
            chroot_path: chroot_path,
            build_dir: build_dir,
            crates_io_index_path: crates_io_index_path,
            logs_path: logs_path,

            chroot_user: "onur".to_string(),

            keep_build_directory: false,
            skip_if_exist: false,
            skip_if_log_exists: false,
            debug: false,
        }
    }
}


impl fmt::Debug for DocBuilder {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,
               "DocBuilder {{ destination: {:?}, chroot_path: {:?}, chroot_user_home_dir: {:?}, \
               crates_io_index_path: {:?}, logs_path: {:?}, chroot_user: {:?}, \
                keep_build_directory: {:?}, skip_if_exist: {:?}, skip_if_log_exists: {:?}, debug: \
                {:?} }}",
                self.destination,
                self.chroot_path,
                self.build_dir,
                self.crates_io_index_path,
                self.logs_path,
                self.chroot_user,
                self.keep_build_directory,
                self.skip_if_exist,
                self.skip_if_log_exists,
                self.debug)
    }
}


impl DocBuilder {
    /// This functions reads files in crates.io-index and tries to build
    /// documentation for crates.
    pub fn build_doc_for_every_crate(&self) {
        self.build_doc_for_crate_path(&self.crates_io_index_path);
    }

    fn build_doc_for_crate_path(&self, path: &PathBuf) {
        for dir in path.read_dir().unwrap() {

            let path = dir.unwrap().path();

            // skip files under .git and config.json
            if path.to_str().unwrap().contains(".git") ||
                path.file_name().unwrap() == "config.json" {
                    continue;
                }

            if path.is_dir() {
                self.build_doc_for_crate_path(&path);
                continue
            }

            // FIXME: check errors here
            let crte = crte::Crate::from_cargo_index_file(path);
            self.build_doc_for_crate(&crte.unwrap());
        }
    }


    /// Builds documentation for crate
    ///
    /// This function will try to build documentation for every version of crate
    pub fn build_doc_for_crate(&self, crte: &crte::Crate) {
        for i in 0..crte.versions.len() {
            self.build_doc_for_crate_version(crte, i);
        }
    }


    fn open_log_for_crate(&self, crte: &crte::Crate, version_index: usize) -> Option<fs::File> {

        // Create a directory in logs folder
        let mut log_file_path = PathBuf::from(&self.logs_path);
        log_file_path.push(&crte.name);
        fs::create_dir(&log_file_path).unwrap();

        log_file_path.push(format!("{}-{}.log",
                                   crte.name, crte.versions[version_index]));

        // FIXME: We are getting panic if file is already exist
        match fs::File::create(log_file_path) {
            Ok(f) => Some(f),
            Err(e) => panic!(e)
        }
    }


    /// Download local dependencies from crate root and place them into right place
    ///
    /// Some packages have local dependencies defined in Cargo.toml
    ///
    /// This function is intentionall written verbose
    fn download_dependencies(&self, root_dir: &PathBuf) -> Result<(), io::Error> {

        let mut cargo_toml_path = PathBuf::from(&root_dir);
        cargo_toml_path.push("Cargo.toml");

        // we are just returning on any error
        println!("CARGO TOML PATH {:?}", cargo_toml_path);

        let mut cargo_toml_fh = try!(fs::File::open(cargo_toml_path));
        let mut cargo_toml_content = String::new();
        try!(cargo_toml_fh.read_to_string(&mut cargo_toml_content));

        toml::Parser::new(&cargo_toml_content[..]).parse().as_ref()
            .and_then(|cargo_toml| cargo_toml.get("dependencies"))
            .and_then(|dependencies| dependencies.as_table())
            .and_then(|dependencies_table| get_local_dependencies(dependencies_table))
            .map(|local_dependencies| self.handle_local_dependencies(local_dependencies, &root_dir));

        Ok(())
    }


    /// Handles local dependencies
    fn handle_local_dependencies(&self,
                                 local_dependencies: Vec<(crte::Crate, String)>,
                                 root_dir: &PathBuf) -> Result<(), String> {
        for local_dependency in local_dependencies {
            let crte = local_dependency.0;

            let mut path = PathBuf::from(&root_dir);
            path.push(local_dependency.1);

            // make sure path exists
            if !path.exists() {
                fs::create_dir_all(&path);
            }

            try!(self.download_latest_version_of_crate(&crte));
            try!(self.extract_crate(&crte, 0));

            let crte_download_dir = PathBuf::from(format!("{}/{}-{}",
                                                          self.build_dir.to_str().unwrap(),
                                                          crte.name, crte.versions[0]));

            if crte_download_dir.exists() {
                println!("CRATE DOWNLOAD DIR EXISTS YUPPI {}",
                         &crte_download_dir.to_str().unwrap());
            }


            // self.extract_crate will extract crate into build_dir
            // Copy files to proper location
            copy_files(&crte_download_dir, &path);
        }

        Ok(())
    }


    /// Returns package folder inside build directory
    fn crate_root_dir(&self, crte: &crte::Crate, version_index: usize) -> PathBuf {
        let mut package_root = PathBuf::from(&self.build_dir);
        package_root.push(format!("{}-{}", &crte.name, &crte.versions[version_index]));
        package_root
    }


    /// Builds documentation for crate
    ///
    /// This operation involves following process:
    ///
    /// * Cleaning up build directory
    /// * Downloading crate
    /// * Extracting it into build directory (chroot dir home directory)
    /// * Building crate documentation with chroot
    /// * Checking build directory for if crate actually has any documentation
    /// * Copying crate documentation into destination path
    /// * Cleaning up build directory
    /// * Removing downloaded crate file
    pub fn build_doc_for_crate_version(&self, crte: &crte::Crate, version_index: usize) {

        let package_root = self.crate_root_dir(&crte, version_index);

        // TODO try to replace noob style logging
        let mut log_file = self.open_log_for_crate(crte, version_index).unwrap();

        println!("Building documentation for {}-{}", crte.name, crte.versions[version_index]);
        write!(&mut log_file,
               "Building documentation for {}-{}\n",
               crte.name, crte.versions[version_index]).unwrap();

        // Download crate
        write!(&mut log_file, "Downloading crate\n").unwrap();;
        if let Err(output) = self.download_crate(&crte, version_index) {
            write!(&mut log_file, "Failed to download crate: {}", output).unwrap();
            return;
        }

        // Extract crate
        write!(&mut log_file, "Extracting crate\n").unwrap();
        if let Err(output) = self.extract_crate(&crte, version_index) {
            write!(&mut log_file, "Failed to extract crate\n").unwrap();
            write!(&mut log_file, "{}\n", output).unwrap();
            return;
        }

        self.download_dependencies(&package_root);
    }



    /// Generates download url
    ///
    /// By default crates.io is using:
    /// https://crates.io/api/v1/crates/$crate/$version/download
    /// But I believe this url is increasing download count and this program is
    /// downloading alot during development. I am using redirected url.
    fn generate_download_url(&self, crte: &crte::Crate, version_index: usize) -> String {
        format!("https://crates-io.s3-us-west-1.amazonaws.com/crates/{}/{}-{}.crate",
                crte.name,
                crte.name,
                crte.versions[version_index])
    }


    /// Downloads crate
    fn download_crate(&self, crte: &crte::Crate, version_index: usize) -> Result<String, String> {
        let url = self.generate_download_url(&crte, version_index);
        // Use wget for now
        command_result(Command::new("wget")
                       .arg("-c")
                       .arg("--content-disposition")
                       .arg(url)
                       .output()
                       .unwrap())
    }


    /// Download latest version of crate
    fn download_latest_version_of_crate(&self, crte: &crte::Crate) -> Result<String, String> {
        // TODO: it might be better to check crte.versions len here
        self.download_crate(&crte, 0)
    }


    /// Extracts crate into build directory
    fn extract_crate(&self, crte: &crte::Crate, version_index: usize) -> Result<String, String> {

        let crate_name = format!("{}-{}.crate", &crte.name, &crte.versions[version_index]);
        command_result(Command::new("tar")
                       .arg("-C")
                       .arg(&self.build_dir)
                       .arg("-xzvf")
                       .arg(crate_name)
                       .output()
                       .unwrap())
    }

}



/// a simple function to capture command output
fn command_result(output: Output) -> Result<String, String> {
    let mut command_out = String::from_utf8_lossy(&output.stdout).into_owned();
    command_out.push_str(&String::from_utf8_lossy(&output.stderr).into_owned()[..]);
    match output.status.success() {
        true => Ok(command_out),
        false => Err(command_out)
    }
}


/// Get's local_dependencies from dependencies_table
fn get_local_dependencies(dependencies_table: &collections::BTreeMap<String, toml::Value>) ->
Option<Vec<(crte::Crate, String)>>  {

    let mut local_dependencies: Vec<(crte::Crate, String)> = Vec::new();

    for key in dependencies_table.keys() {

        dependencies_table.get(key)
            .and_then(|key_val| key_val.as_table())
            .map(|key_table| {
                key_table.get("path").and_then(|p| p.as_str()).map(|path| {
                    key_table.get("version").and_then(|p| p.as_str())
                        .map(|version| {
                            let dep_crate = crte::Crate::new(key.clone(),
                            vec![version.to_string()]);
                            local_dependencies.push((dep_crate, path.to_string()));
                        });
                });
            });

    }
    Some(local_dependencies)
}



enum CopyFilesError {
    FailedToReadSourceDir(String),
}


// My first error implementation
impl convert::From for CopyFilesError {
    fn from(err: io::Error) -> CopyFilesError {
        match err {
            EntryMissing => "entry is missing",
            BadFileFormat => "a bad file format encountered",
            CouldNotOpenFile => "unable to open file",
            InternalError => "an internal error occurred",
        }
    }
}


fn copy_files(source: &PathBuf, destination: &PathBuf) -> Result<(), CopyFilesError> {

    for file in source.read_dir().unwrap() {

        // FIXME: unwrap'ing here is probably not a good idea
        let file = file.unwrap();
        let mut destination_full_path = PathBuf::from(&destination);
        destination_full_path.push(file.file_name());

        println!("SOURCE: {:#?}", file.path());
        println!("DESTIN: {:#?}", destination_full_path);

        if file.metadata().unwrap().is_dir() {
            try!(fs::create_dir(&destination_full_path));
            copy_files(&file.path(), &destination_full_path);
        } else {
            try!(fs::copy(&file.path(), &destination_full_path));
        }

    }

}
