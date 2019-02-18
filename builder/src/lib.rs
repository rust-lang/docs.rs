//! Crate documentation builder

pub mod build_metadata;
pub mod build_result;
pub mod builder;
pub mod package_metadata;
pub mod utils;

use failure::Error;
pub type Result<T> = std::result::Result<T, Error>;

use std::path::Path;

use build_metadata::BuildMetadata;
use builder::Builder;

/// This target will be used for building crates that don't have
/// default-target definition in [packages.metadata.docs.rs] in their Cargo.toml.
pub const DEFAULT_TARGET: &str = env!("TARGET");

/// List of targets supported by docs.rs
pub const TARGETS: [&str; 6] = [
    "i686-apple-darwin",
    "i686-pc-windows-msvc",
    "i686-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
];

/// Builds a docsrs documentation package. Main entry point of this crate.
pub fn build_doc(name: &str, exact_version: &str, target_dir: impl AsRef<Path>) -> Result<()> {
    // add equal in front of version to make it semver compatible
    let version = format!("={}", exact_version);
    let builder = Builder::new(name, Some(&version), &target_dir)?;
    let pkg = builder.get_package()?;
    let metadata = builder.get_metadata(&pkg)?;

    // clean documentation directories
    builder.clean()?;

    // install system dependencies
    builder.install_system_dependencies(&metadata)?;

    // first build crate for default target
    // this will be our default documentation
    let default_target_res = builder.build_doc(
        &pkg,
        &metadata,
        metadata
            .default_target
            .as_ref()
            .map(String::as_ref)
            .unwrap_or(DEFAULT_TARGET),
    );

    // build documentation for all targets if default documentation
    // built successfully and get list of successfully targets.
    let successfully_targets = if default_target_res.build_status() {
        TARGETS
            .iter()
            .filter(|&&t| t != DEFAULT_TARGET)
            .map(|t| t.to_string())
            .filter(|t| builder.build_doc(&pkg, &metadata, t).build_status())
            .collect()
    } else {
        Vec::new()
    };

    // create build metadata and docsrs documentation package
    BuildMetadata::new(default_target_res, successfully_targets)?
        .create_package(&target_dir, &pkg)?;

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn test_build_doc() {
        let target_dir = tempdir().unwrap();
        let build_res = build_doc("acme-client", "0.0.0", &target_dir);

        assert!(build_res.is_ok());
        assert!(Path::new(target_dir.path()).join("docsrs.json").exists());
        assert!(Path::new(target_dir.path())
            .join("acme-client-0.0.0.zip")
            .exists());
        assert!(Path::new(target_dir.path())
            .join(DEFAULT_TARGET)
            .join("doc")
            .join("acme_client")
            .join("index.html")
            .exists());

        for target in TARGETS.iter() {
            assert!(Path::new(target_dir.path()).join(target).exists());
        }
    }
}
