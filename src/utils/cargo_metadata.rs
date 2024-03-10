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
    pub(crate) fn load_from_rustwide(
        workspace: &Workspace,
        toolchain: &Toolchain,
        source_dir: &Path,
    ) -> Result<Self> {
        let res = Command::new(workspace, toolchain.cargo())
            .args(&["metadata", "--format-version", "1"])
            .cd(source_dir)
            .log_output(false)
            .run_capture()?;
        let [metadata] = res.stdout_lines() else {
            bail!("invalid output returned by `cargo metadata`")
        };
        Self::load_from_metadata(metadata)
    }

    #[cfg(test)]
    pub(crate) fn load_from_host_path(source_dir: &Path) -> Result<Self> {
        let res = std::process::Command::new("cargo")
            .args(["metadata", "--format-version", "1", "--offline"])
            .current_dir(source_dir)
            .output()?;
        let status = res.status;
        if !status.success() {
            let stderr = std::str::from_utf8(&res.stderr).unwrap_or("");
            bail!("error returned by `cargo metadata`: {status}\n{stderr}")
        }
        Self::load_from_metadata(std::str::from_utf8(&res.stdout)?)
    }

    pub(crate) fn load_from_metadata(metadata: &str) -> Result<Self> {
        let metadata = serde_json::from_str::<DeserializedMetadata>(metadata)?;
        let root = metadata.resolve.root;
        Ok(CargoMetadata {
            root: metadata
                .packages
                .into_iter()
                .find(|pkg| pkg.id == root)
                .context("metadata.packages missing root package")?,
        })
    }

    pub(crate) fn root(&self) -> &Package {
        &self.root
    }
}

#[derive(Debug, Deserialize, Serialize, Default)]
pub(crate) struct Package {
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

    pub(crate) fn package_name(&self) -> String {
        self.library_name()
            .unwrap_or_else(|| self.normalize_package_name(&self.targets[0].name))
    }

    pub(crate) fn library_name(&self) -> Option<String> {
        self.library_target()
            .map(|target| self.normalize_package_name(&target.name))
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Target {
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

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct Dependency {
    pub(crate) name: String,
    pub(crate) req: String,
    pub(crate) kind: Option<String>,
    pub(crate) rename: Option<String>,
    pub(crate) optional: bool,
}

impl Dependency {
    #[cfg(test)]
    pub fn new(name: String, req: String) -> Dependency {
        Dependency {
            name,
            req,
            kind: None,
            rename: None,
            optional: false,
        }
    }

    #[cfg(test)]
    pub fn set_optional(mut self, optional: bool) -> Self {
        self.optional = optional;
        self
    }
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
