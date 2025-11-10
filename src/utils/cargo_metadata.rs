use crate::{db::types::version::Version, error::Result};
use anyhow::bail;
use cargo_metadata::{CrateType, Metadata, Target};
use rustwide::{Toolchain, Workspace, cmd::Command};
use std::path::Path;

pub(crate) fn load_cargo_metadata_from_rustwide(
    workspace: &Workspace,
    toolchain: &Toolchain,
    source_dir: &Path,
) -> Result<Metadata> {
    let res = Command::new(workspace, toolchain.cargo())
        .args(&["metadata", "--format-version", "1"])
        .cd(source_dir)
        .log_output(false)
        .run_capture()?;
    let [metadata] = res.stdout_lines() else {
        bail!("invalid output returned by `cargo metadata`")
    };

    Ok(serde_json::from_str(metadata)?)
}

#[cfg(test)]
pub(crate) fn load_cargo_metadata_from_host_path(source_dir: &Path) -> Result<Metadata> {
    let res = std::process::Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--offline"])
        .current_dir(source_dir)
        .output()?;
    let status = res.status;
    if !status.success() {
        let stderr = std::str::from_utf8(&res.stderr).unwrap_or("");
        bail!("error returned by `cargo metadata`: {status}\n{stderr}")
    }
    Ok(serde_json::from_slice(&res.stdout)?)
}

pub(crate) trait MetadataExt {
    fn root(&self) -> &cargo_metadata::Package;
}

impl MetadataExt for cargo_metadata::Metadata {
    fn root(&self) -> &cargo_metadata::Package {
        self.root_package().as_ref().expect("missing root package")
    }
}

pub(crate) trait PackageExt {
    fn library_target(&self) -> Option<&Target>;
    fn is_library(&self) -> bool;
    fn normalize_package_name(&self, name: &str) -> String;
    fn package_name(&self) -> String;
    fn library_name(&self) -> Option<String>;
    fn version(&self) -> Version;
}

impl PackageExt for cargo_metadata::Package {
    fn library_target(&self) -> Option<&Target> {
        self.targets.iter().find(|target| {
            target
                .crate_types
                .iter()
                .any(|kind| kind != &CrateType::Bin)
        })
    }

    fn is_library(&self) -> bool {
        self.library_target().is_some()
    }

    fn normalize_package_name(&self, name: &str) -> String {
        name.replace('-', "_")
    }

    fn package_name(&self) -> String {
        self.library_name().unwrap_or_else(|| {
            self.targets
                .first()
                .map(|t| self.normalize_package_name(&t.name))
                .unwrap_or_default()
        })
    }

    fn library_name(&self) -> Option<String> {
        self.library_target()
            .map(|target| self.normalize_package_name(&target.name))
    }

    fn version(&self) -> Version {
        self.version.clone().into()
    }
}
