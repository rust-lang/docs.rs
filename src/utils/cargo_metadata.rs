use cargo::core::Package as CargoLibPackage;
use error::Result;
use rustwide::{cmd::Command, Toolchain, Workspace};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

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

    // All of this is very hacky, but it's just needed to cleanly transition the code from cargo
    // lib to cargo metadata.
    pub(crate) fn from_cargo_lib_package(pkg: &CargoLibPackage) -> Self {
        let id = pkg.package_id().to_string();
        CargoMetadata {
            packages: ::std::iter::once((
                id.clone(),
                Package {
                    id: id.clone(),
                    name: pkg.name().as_str().to_string(),
                    version: pkg.version().to_string(),
                    license: pkg.manifest().metadata().license.clone(),
                    repository: pkg.manifest().metadata().repository.clone(),
                    homepage: pkg.manifest().metadata().homepage.clone(),
                    description: pkg.manifest().metadata().description.clone(),
                    documentation: pkg.manifest().metadata().documentation.clone(),
                    targets: pkg
                        .manifest()
                        .targets()
                        .iter()
                        .map(|target| Target {
                            name: target.name().to_string(),
                            kind: if target.is_lib() {
                                vec!["lib".into()]
                            } else {
                                vec![]
                            },
                            src_path: target.src_path().path().map(|p| p.into()),
                        })
                        .collect(),
                    dependencies: pkg
                        .manifest()
                        .dependencies()
                        .iter()
                        .map(|dep| Dependency {
                            name: dep.package_name().to_string(),
                            req: dep.version_req().to_string(),
                            kind: match dep.kind() {
                                ::cargo::core::dependency::Kind::Normal => None,
                                ::cargo::core::dependency::Kind::Development => Some("dev".into()),
                                ::cargo::core::dependency::Kind::Build => Some("build".into()),
                            },
                        })
                        .collect(),
                    readme: pkg.manifest().metadata().readme.clone(),
                    keywords: pkg.manifest().metadata().keywords.clone(),
                    authors: pkg.manifest().metadata().authors.clone(),
                },
            ))
            .collect(),
            deps_graph: HashMap::new(),
            root_id: id,
        }
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

#[derive(RustcDecodable)]
pub(crate) struct Target {
    pub(crate) name: String,
    pub(crate) kind: Vec<String>,
    pub(crate) src_path: Option<PathBuf>,
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
