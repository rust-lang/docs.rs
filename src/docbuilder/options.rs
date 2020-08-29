use crate::error::Result;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct DocBuilderOptions {
    pub(crate) prefix: PathBuf,
    pub(crate) registry_index_path: PathBuf,
    pub skip_if_exists: bool,
}

impl DocBuilderOptions {
    pub fn new(prefix: PathBuf, registry_index_path: PathBuf) -> DocBuilderOptions {
        DocBuilderOptions {
            prefix,
            registry_index_path,

            skip_if_exists: false,
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
