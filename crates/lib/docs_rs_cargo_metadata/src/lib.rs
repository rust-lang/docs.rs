pub mod db;

use anyhow::{Context, Result};
use docs_rs_database::types::version::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[cfg(test)]
use std::path::Path;

pub struct CargoMetadata {
    root: Package,
}

impl CargoMetadata {
    #[cfg(test)]
    pub fn load_from_host_path(source_dir: &Path) -> Result<Self> {
        let res = std::process::Command::new("cargo")
            .args(["metadata", "--format-version", "1", "--offline"])
            .current_dir(source_dir)
            .output()?;
        let status = res.status;
        if !status.success() {
            let stderr = std::str::from_utf8(&res.stderr).unwrap_or("");
            anyhow::bail!("error returned by `cargo metadata`: {status}\n{stderr}")
        }
        Self::load_from_metadata(std::str::from_utf8(&res.stdout)?)
    }

    pub fn load_from_metadata(metadata: &str) -> Result<Self> {
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

    pub fn root(&self) -> &Package {
        &self.root
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub version: Version,
    pub license: Option<String>,
    pub repository: Option<String>,
    pub homepage: Option<String>,
    pub description: Option<String>,
    pub documentation: Option<String>,
    pub dependencies: Vec<Dependency>,
    pub targets: Vec<Target>,
    pub readme: Option<String>,
    pub keywords: Vec<String>,
    pub features: HashMap<String, Vec<String>>,
}

impl Package {
    fn library_target(&self) -> Option<&Target> {
        self.targets
            .iter()
            .find(|target| target.crate_types.iter().any(|kind| kind != "bin"))
    }

    pub fn is_library(&self) -> bool {
        self.library_target().is_some()
    }

    fn normalize_package_name(&self, name: &str) -> String {
        name.replace('-', "_")
    }

    pub fn package_name(&self) -> String {
        self.library_name().unwrap_or_else(|| {
            self.targets
                .first()
                .map(|t| self.normalize_package_name(&t.name))
                .unwrap_or_default()
        })
    }

    pub fn library_name(&self) -> Option<String> {
        self.library_target()
            .map(|target| self.normalize_package_name(&target.name))
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Target {
    pub name: String,
    #[cfg(not(test))]
    crate_types: Vec<String>,
    #[cfg(test)]
    pub crate_types: Vec<String>,
    pub src_path: Option<String>,
}

impl Target {
    #[cfg(test)]
    pub fn dummy_lib(name: String, src_path: Option<String>) -> Self {
        Target {
            name,
            crate_types: vec!["lib".into()],
            src_path,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Dependency {
    pub name: String,
    pub req: VersionReq,
    pub kind: Option<String>,
    pub rename: Option<String>,
    pub optional: bool,
}

impl bincode::Encode for Dependency {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> Result<(), bincode::error::EncodeError> {
        let Self {
            name,
            req,
            kind,
            rename,
            optional,
        } = self;
        name.encode(encoder)?;
        // FIXME: VersionReq does not implement Encode, so we serialize it to string
        // Could be fixable by wrapping VersionReq in a newtype
        req.to_string().encode(encoder)?;
        kind.encode(encoder)?;
        rename.encode(encoder)?;
        optional.encode(encoder)?;
        Ok(())
    }
}

impl Dependency {
    #[cfg(test)]
    pub fn new(name: String, req: VersionReq) -> Dependency {
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
