use error::Result;
use rustwide::{cmd::Command, Toolchain, Workspace};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub(crate) struct CargoMetadata {
    packages: HashMap<String, Package>,
    deps_graph: HashMap<String, HashSet<String>>,
    root_id: String,
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

        let mut iter = res.stdout_lines().iter();
        let metadata = if let (Some(serialized), None) = (iter.next(), iter.next()) {
            ::rustc_serialize::json::decode::<DeserializedMetadata>(serialized)?
        } else {
            return Err(::failure::err_msg(
                "invalid output returned by `cargo metadata`",
            ));
        };

        // Convert from Vecs to HashMaps and HashSets to get more efficient lookups
        Ok(CargoMetadata {
            packages: metadata
                .packages
                .into_iter()
                .map(|pkg| {
                    (
                        pkg.id,
                        Package {
                            name: pkg.name,
                            version: pkg.version,
                        },
                    )
                })
                .collect(),
            deps_graph: metadata
                .resolve
                .nodes
                .into_iter()
                .map(|node| (node.id, node.deps.into_iter().map(|d| d.pkg).collect()))
                .collect(),
            root_id: metadata.resolve.root,
        })
    }

    pub(crate) fn root_dependencies(&self) -> Vec<&Package> {
        let ids = &self.deps_graph[&self.root_id];
        self.packages
            .iter()
            .filter(|(id, _pkg)| ids.contains(id.as_str()))
            .map(|(_id, pkg)| pkg)
            .collect()
    }

    pub(crate) fn root(&self) -> &Package {
        &self.packages[&self.root_id]
    }
}

pub(crate) struct Package {
    name: String,
    version: String,
}

impl Package {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn version(&self) -> &str {
        &self.version
    }
}

#[derive(RustcDecodable)]
struct DeserializedMetadata {
    packages: Vec<DeserializedPackage>,
    resolve: DeserializedResolve,
}

#[derive(RustcDecodable)]
struct DeserializedPackage {
    id: String,
    name: String,
    version: String,
}

#[derive(RustcDecodable)]
struct DeserializedResolve {
    root: String,
    nodes: Vec<DeserializedResolveNode>,
}

#[derive(RustcDecodable)]
struct DeserializedResolveNode {
    id: String,
    deps: Vec<DeserializedResolveDep>,
}

#[derive(RustcDecodable)]
struct DeserializedResolveDep {
    pkg: String,
}
