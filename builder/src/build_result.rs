use crate::builder::source_path;
use crate::package_metadata::PackageMetadata;
use crate::Result;
use cargo::core::Package;
use failure::err_msg;
use std::path::{Path, PathBuf};

pub struct BuildResult<'a> {
    package: &'a Package,
    metadata: &'a PackageMetadata,
    target: &'a str,
    target_dir: &'a PathBuf,
    build_status: bool,
}

impl<'a> BuildResult<'a> {
    pub fn new(
        package: &'a Package,
        metadata: &'a PackageMetadata,
        target: &'a str,
        target_dir: &'a PathBuf,
        build_status: bool,
    ) -> BuildResult<'a> {
        BuildResult {
            package,
            metadata,
            target,
            target_dir,
            build_status,
        }
    }

    /// Checks a package build directory to determine if package have docs
    pub fn have_docs(&self) -> bool {
        let crate_doc_path = Path::new(&self.target_dir)
            .join(self.target)
            .join("doc")
            .join(
                self.package.targets()[0]
                    .name()
                    .replace("-", "_")
                    .to_string(),
            );
        crate_doc_path.exists()
    }

    /// Checks if package have examples
    pub fn have_examples(&self) -> Result<bool> {
        let path = source_path(self.package)
            .ok_or_else(|| err_msg("Source path not available"))?
            .join("examples");
        Ok(path.exists() && path.is_dir())
    }

    pub fn is_library(&self) -> bool {
        match *self.package.targets()[0].kind() {
            cargo::core::TargetKind::Lib(_) => true,
            _ => false,
        }
    }

    pub fn build_status(&self) -> bool {
        self.build_status
    }

    pub fn pkg(&self) -> &'a Package {
        self.package
    }

    pub fn metadata(&self) -> &'a PackageMetadata {
        self.metadata
    }
}
