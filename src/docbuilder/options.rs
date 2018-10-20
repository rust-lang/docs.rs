

use std::{env, fmt};
use std::path::PathBuf;
use error::Result;
use failure::err_msg;

#[derive(Clone)]
pub struct DocBuilderOptions {
    pub keep_build_directory: bool,
    pub prefix: PathBuf,
    pub destination: PathBuf,
    pub chroot_path: PathBuf,
    pub chroot_user: String,
    pub container_name: String,
    pub crates_io_index_path: PathBuf,
    pub sources_path: PathBuf,
    pub skip_if_exists: bool,
    pub skip_if_log_exists: bool,
    pub skip_oldest_versions: bool,
    pub build_only_latest_version: bool,
    pub debug: bool,
}



impl Default for DocBuilderOptions {
    fn default() -> DocBuilderOptions {

        let cwd = env::current_dir().unwrap();

        let (prefix, destination, chroot_path, crates_io_index_path, sources_path) =
            generate_paths(cwd);

        DocBuilderOptions {
            prefix: prefix,
            destination: destination,
            chroot_path: chroot_path,
            crates_io_index_path: crates_io_index_path,
            sources_path: sources_path,

            chroot_user: "cratesfyi".to_string(),
            container_name: "cratesfyi-container".to_string(),

            keep_build_directory: false,
            skip_if_exists: false,
            skip_if_log_exists: false,
            skip_oldest_versions: false,
            build_only_latest_version: false,
            debug: false,
        }
    }
}


impl fmt::Debug for DocBuilderOptions {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f,
               "DocBuilderOptions {{ destination: {:?}, chroot_path: {:?}, \
                crates_io_index_path: {:?}, \
                sources_path: {:?}, container_name: {:?}, chroot_user: {:?}, \
                keep_build_directory: {:?}, skip_if_exists: {:?}, \
                skip_if_log_exists: {:?}, debug: {:?} }}",
               self.destination,
               self.chroot_path,
               self.crates_io_index_path,
               self.sources_path,
               self.container_name,
               self.chroot_user,
               self.keep_build_directory,
               self.skip_if_exists,
               self.skip_if_log_exists,
               self.debug)
    }
}


impl DocBuilderOptions {
    /// Creates new DocBuilderOptions from prefix
    pub fn from_prefix(prefix: PathBuf) -> DocBuilderOptions {
        let (prefix, destination, chroot_path, crates_io_index_path, sources_path) =
            generate_paths(prefix);
        DocBuilderOptions {
            prefix: prefix,
            destination: destination,
            chroot_path: chroot_path,
            crates_io_index_path: crates_io_index_path,
            sources_path: sources_path,

            ..Default::default()
        }
    }


    pub fn check_paths(&self) -> Result<()> {
        if !self.destination.exists() {
            return Err(err_msg("Destination path not exists"));
        }
        if !self.chroot_path.exists() {
            return Err(err_msg("Chroot path not exists"));
        }
        if !self.crates_io_index_path.exists() {
            return Err(err_msg("crates.io-index path not exists"));
        }
        if !self.crates_io_index_path.exists() {
            return Err(err_msg("Logs path not exists"));
        }
        Ok(())
    }
}



fn generate_paths(prefix: PathBuf) -> (PathBuf, PathBuf, PathBuf, PathBuf, PathBuf) {

    let destination = PathBuf::from(&prefix).join("documentations");
    let chroot_path = PathBuf::from(&prefix).join("cratesfyi-container/rootfs");
    let crates_io_index_path = PathBuf::from(&prefix).join("crates.io-index");
    let sources_path = PathBuf::from(&prefix).join("sources");

    (prefix, destination, chroot_path, crates_io_index_path, sources_path)
}
