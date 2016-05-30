

use std::{env, fmt};
use std::path::PathBuf;


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



// This error only occurs if check_dirs fails
pub enum DocBuilderPathError {
    DestinationPathNotExists,
    ChrootPathNotExists,
    BuildDirectoryNotExists,
    CratesIoIndexPathNotExists,
    LogsPathNotExists,
}


impl fmt::Debug for DocBuilderPathError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            DocBuilderPathError::DestinationPathNotExists =>
                write!(f, "Destination path not exists"),
            DocBuilderPathError::ChrootPathNotExists =>
                write!(f, "Chroot path not exists"),
            DocBuilderPathError::BuildDirectoryNotExists =>
                write!(f, "Build directory path not exists"),
            DocBuilderPathError::CratesIoIndexPathNotExists =>
                write!(f, "crates.io-index path not exists"),
            DocBuilderPathError::LogsPathNotExists =>
                write!(f, "Logs path not exists"),
        }
    }
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
                sources_path: {:?}, chroot_user: {:?}, \
                keep_build_directory: {:?}, skip_if_exists: {:?}, \
                skip_if_log_exists: {:?}, debug: {:?} }}",
                self.destination,
                self.chroot_path,
                self.crates_io_index_path,
                self.sources_path,
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

            .. Default::default()
        }
    }


    pub fn check_paths(&self) -> Result<(), DocBuilderPathError> {
        if !self.destination.exists() {
            return Err(DocBuilderPathError::DestinationPathNotExists)
        }
        if !self.chroot_path.exists() {
            return Err(DocBuilderPathError::ChrootPathNotExists)
        }
        if !self.crates_io_index_path.exists() {
            return Err(DocBuilderPathError::CratesIoIndexPathNotExists)
        }
        if !self.crates_io_index_path.exists() {
            return Err(DocBuilderPathError::LogsPathNotExists)
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
