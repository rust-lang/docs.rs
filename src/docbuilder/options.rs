use crate::error::Result;
use std::path::PathBuf;
use std::{env, fmt};

#[derive(Clone)]
pub struct DocBuilderOptions {
    pub keep_build_directory: bool,
    pub prefix: PathBuf,
    pub registry_index_path: PathBuf,
    pub skip_if_exists: bool,
    pub skip_if_log_exists: bool,
    pub skip_oldest_versions: bool,
    pub build_only_latest_version: bool,
    pub debug: bool,
}

impl Default for DocBuilderOptions {
    fn default() -> DocBuilderOptions {
        let cwd = env::current_dir().unwrap();

        let (prefix, registry_index_path) = generate_paths(cwd);

        DocBuilderOptions {
            prefix,
            registry_index_path,

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
        write!(
            f,
            "DocBuilderOptions {{ \
                registry_index_path: {:?}, \
                keep_build_directory: {:?}, skip_if_exists: {:?}, \
                skip_if_log_exists: {:?}, debug: {:?} }}",
            self.registry_index_path,
            self.keep_build_directory,
            self.skip_if_exists,
            self.skip_if_log_exists,
            self.debug
        )
    }
}

impl DocBuilderOptions {
    /// Creates new DocBuilderOptions from prefix
    pub fn from_prefix(prefix: PathBuf) -> DocBuilderOptions {
        let (prefix, registry_index_path) = generate_paths(prefix);

        DocBuilderOptions {
            prefix,
            registry_index_path,
            ..Default::default()
        }
    }

    pub fn check_paths(&self) -> Result<()> {
        if !self.registry_index_path.exists() {
            failure::bail!(
                "registry index path '{}' does not exist",
                self.registry_index_path.display()
            );
        }

        Ok(())
    }
}

fn generate_paths(prefix: PathBuf) -> (PathBuf, PathBuf) {
    let registry_index_path = PathBuf::from(&prefix).join("crates.io-index");

    (prefix, registry_index_path)
}
