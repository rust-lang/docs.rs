use crate::error::Result;
use anyhow::{bail, Context};
use rustwide::{cmd::Command, Toolchain, Workspace};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

pub(crate) struct CargoMetadata {
    root: Package,
}

impl CargoMetadata {
    pub(crate) fn load(
        workspace: &Workspace,
        toolchain: &Toolchain,
        source_dir: &Path,
    ) -> Result<Self> {
        let res = Command::new(workspace, toolchain.cargo())
            .args(&["metadata", "--format-version", "1"])
            .cd(source_dir)
            .log_output(false)
            .run_capture()?;

        let iter = res.stdout_lines().iter();
        Ok(CargoMetadata {
            root: Package::from_lines(iter)?,
        })
    }

    pub fn root(&self) -> &Package {
        &self.root
    }
}

#[derive(Deserialize, Serialize, Default)]
pub struct Package {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) license: Option<String>,
    pub(crate) repository: Option<String>,
    pub(crate) homepage: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) documentation: Option<String>,
    pub(crate) dependencies: Vec<Dependency>,
    pub(crate) targets: Vec<Target>,
    pub(crate) readme: Option<String>,
    pub(crate) keywords: Vec<String>,
    pub(crate) features: HashMap<String, Vec<String>>,
}

impl Package {
    fn library_target(&self) -> Option<&Target> {
        self.targets
            .iter()
            .find(|target| target.crate_types.iter().any(|kind| kind != "bin"))
    }

    pub(crate) fn is_library(&self) -> bool {
        self.library_target().is_some()
    }

    fn normalize_package_name(&self, name: &str) -> String {
        name.replace('-', "_")
    }

    /// Returns the package name without normalization.
    pub fn as_raw_name(&self) -> &str {
        &self.name
    }

    /// Returns the package version.
    pub fn package_version(&self) -> &str {
        &self.version
    }

    pub fn package_name(&self) -> String {
        self.library_name()
            .unwrap_or_else(|| self.normalize_package_name(&self.targets[0].name))
    }

    pub(crate) fn library_name(&self) -> Option<String> {
        self.library_target()
            .map(|target| self.normalize_package_name(&target.name))
    }

    /// Deserialize metadata from lines.
    pub fn from_lines<'a, T: AsRef<str>>(mut iter: impl Iterator<Item = T> + 'a) -> Result<Self> {
        let metadata = if let (Some(serialized), None) = (iter.next(), iter.next()) {
            serde_json::from_str::<DeserializedMetadata>(serialized.as_ref())?
        } else {
            bail!("invalid output returned by `cargo metadata`");
        };

        let root = metadata.resolve.root;
        metadata
            .packages
            .into_iter()
            .find(|pkg| pkg.id == root)
            .context("failed to find the root package")
    }
}

#[derive(Deserialize, Serialize)]
pub struct Target {
    pub(crate) name: String,
    #[cfg(not(test))]
    crate_types: Vec<String>,
    #[cfg(test)]
    pub(crate) crate_types: Vec<String>,
    pub(crate) src_path: Option<String>,
}

impl Target {
    #[cfg(test)]
    pub(crate) fn dummy_lib(name: String, src_path: Option<String>) -> Self {
        Target {
            name,
            crate_types: vec!["lib".into()],
            src_path,
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct Dependency {
    pub(crate) name: String,
    pub(crate) req: String,
    pub(crate) kind: Option<String>,
    pub(crate) rename: Option<String>,
    pub(crate) optional: bool,
}

#[derive(Deserialize, Serialize)]
struct DeserializedMetadata {
    packages: Vec<Package>,
    resolve: DeserializedResolve,
}

#[derive(Deserialize, Serialize)]
struct DeserializedResolve {
    root: String,
    nodes: Vec<DeserializedResolveNode>,
}

#[derive(Deserialize, Serialize)]
struct DeserializedResolveNode {
    id: String,
    deps: Vec<DeserializedResolveDep>,
}

#[derive(Deserialize, Serialize)]
struct DeserializedResolveDep {
    pkg: String,
}
