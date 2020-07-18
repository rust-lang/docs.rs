use crate::error::Result;
use std::path::PathBuf;

#[derive(Clone, Debug)]
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

impl DocBuilderOptions {
    pub fn new(prefix: PathBuf, registry_index_path: PathBuf) -> DocBuilderOptions {
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
