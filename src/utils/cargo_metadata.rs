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
                .map(|pkg| (pkg.id.clone(), pkg))
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

#[derive(RustcDecodable)]
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
    pub(crate) authors: Vec<String>,
}

impl Package {
    pub(crate) fn is_library(&self) -> bool {
        match self.targets[0].kind.as_slice() {
            &[ref kind] if kind == "lib" || kind == "proc-macro" => true,
            _ => false,
        }
    }
}

#[derive(RustcDecodable)]
pub(crate) struct Target {
    pub(crate) name: String,
    pub(crate) kind: Vec<String>,
    pub(crate) src_path: Option<String>,
}

#[derive(RustcDecodable)]
pub(crate) struct Dependency {
    pub(crate) name: String,
    pub(crate) req: String,
    pub(crate) kind: Option<String>,
}

#[derive(RustcDecodable)]
struct DeserializedMetadata {
    packages: Vec<Package>,
    resolve: DeserializedResolve,
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
