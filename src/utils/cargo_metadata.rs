#[cfg(test)]
pub use raw::Target;
pub use raw::{Dependency, Package};

use crate::error::Result;
use rustwide::{cmd::Command, Toolchain, Workspace};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

const CARGO_METADATA_ARGS: &[&str] = &["metadata", "--format-version", "1"];

pub struct CargoMetadata {
    packages: HashMap<String, Package>,
    dependency_graph: HashMap<String, HashSet<String>>,
    root_package_id: String,
}

impl CargoMetadata {
    pub fn load(workspace: &Workspace, toolchain: &Toolchain, source_dir: &Path) -> Result<Self> {
        let output = Command::new(workspace, toolchain.cargo())
            .args(CARGO_METADATA_ARGS)
            .cd(source_dir)
            .log_output(false)
            .run_capture()?;

        let metadata = {
            let mut stdout = output.stdout_lines().iter();

            if let (Some(serialized), None) = (stdout.next(), stdout.next()) {
                serde_json::from_str::<raw::MetadataRoot>(serialized)?
            } else {
                return Err(::failure::err_msg(
                    "invalid output returned by `cargo metadata`",
                ));
            }
        };

        // Collect all packages into a map by their id
        let packages = metadata
            .packages
            .into_iter()
            .map(|pkg| (pkg.id.clone(), pkg))
            .collect();

        // Collect a dependency graph of package ids to the ids of packages they depend on
        let dependency_graph = metadata
            .resolve
            .nodes
            .into_iter()
            .map(|node| {
                let dependents = node
                    .deps
                    .into_iter()
                    .map(|dependent| dependent.pkg)
                    .collect();

                (node.id, dependents)
            })
            .collect();

        // Get the id of the root package, aka the one we're building
        let root_package_id = metadata.resolve.root;

        Ok(CargoMetadata {
            packages,
            dependency_graph,
            root_package_id,
        })
    }

    /// Get the root package
    pub(crate) fn root(&self) -> &Package {
        self.get_package(&self.root_package_id)
            .expect("the root package does not exist within the given metadata")
    }

    pub(crate) fn get_package(&self, id: &str) -> Option<&Package> {
        self.packages.get(id)
    }

    /// Get all direct dependencies of the root package
    pub(crate) fn root_dependencies(&self) -> Vec<&Package> {
        let ids = self
            .dependency_graph
            .get(&self.root_package_id)
            .expect("the root package does not exist within the given metadata");

        self.packages
            .iter()
            .filter(|(id, _pkg)| ids.contains(id.as_str()))
            .map(|(_id, pkg)| pkg)
            .collect()
    }
}

mod raw {
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;

    #[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct MetadataRoot {
        pub packages: Vec<Package>,
        pub resolve: Resolve,
        /// The members of the current workspace in the format of `{name} {version} ({path})`.
        /// Used to resolve `path`-based dependencies
        pub workspace_members: Vec<String>,
    }

    #[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Resolve {
        pub nodes: Vec<Node>,
        pub root: String,
    }

    #[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Node {
        pub id: String,
        pub dependencies: Vec<String>,
        pub deps: Vec<Dep>,
        pub features: Vec<String>,
    }

    #[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Dep {
        pub name: String,
        pub pkg: String,
        pub dep_kinds: Vec<DepKind>,
    }

    #[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct DepKind {
        pub kind: Option<String>,
        pub target: Option<String>,
    }

    #[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Package {
        pub name: String,
        pub version: String,
        /// The id of the package in the format of `{name} {version} ({path})`.
        /// Use in conjunction with `workspace_members` to resolve `path`-based dependencies
        pub id: String,
        pub license: Option<String>,
        pub license_file: Option<String>,
        pub description: Option<String>,
        pub dependencies: Vec<Dependency>,
        pub targets: Vec<Target>,
        pub features: HashMap<String, Vec<String>>,
        pub authors: Vec<String>,
        pub categories: Vec<String>,
        pub keywords: Vec<String>,
        pub readme: Option<String>,
        pub repository: Option<String>,
        pub homepage: Option<String>,
        pub documentation: Option<String>,
        pub edition: String,
        pub metadata: Option<Metadata>,
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

    #[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Dependency {
        pub name: String,
        #[serde(rename = "req")]
        pub version_requirement: String,
        pub kind: Option<String>,
        pub rename: Option<String>,
        pub optional: bool,
        #[serde(rename = "uses_default_features")]
        pub default_features: bool,
        pub features: Vec<String>,
        pub target: Option<String>,
    }

    #[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub struct Target {
        pub name: String,
        pub kind: Vec<String>,
        #[serde(rename = "crate_types")]
        pub crate_types: Vec<String>,
        pub src_path: Option<String>,
        pub edition: String,
        pub doctest: bool,
        #[serde(default)]
        pub required_features: Vec<String>,
    }

    impl Target {
        #[cfg(test)]
        pub(crate) fn dummy_lib(name: String, src_path: Option<String>) -> Self {
            Target {
                name,
                crate_types: vec!["lib".into()],
                src_path,
                kind: vec!["lib".to_owned()],
                doctest: false,
                edition: "2018".to_owned(),
                required_features: Vec::new(),
            }
        }
    }

    #[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Metadata {
        pub docs: Option<Docs>,
    }

    #[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Docs {
        pub rs: Rs,
    }

    #[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "kebab-case")]
    pub struct Rs {
        #[serde(default)]
        pub targets: Vec<String>,
        #[serde(default)]
        pub features: Vec<String>,
        pub all_features: Option<bool>,
        pub default_target: Option<String>,
        #[serde(default)]
        pub rustdoc_args: Vec<String>,
        pub no_default_features: Option<bool>,
        pub rustc_args: Option<Vec<String>>,
    }
}
