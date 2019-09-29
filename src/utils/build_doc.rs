//! Crate documentation builder
//!
//! This module is extremely similar to cargo install operation, except it's building
//! documentation of a crate and not installing anything.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::env;
use std::sync::Arc;

use cargo::core::{self, SourceId, Dependency, Source, Package, Workspace};
use cargo::core::compiler::{DefaultExecutor, CompileMode, MessageFormat, BuildConfig, Executor};
use cargo::core::package::PackageSet;
use cargo::core::registry::PackageRegistry;
use cargo::core::resolver;
use cargo::core::source::SourceMap;
use cargo::util::{CargoResult, Config, internal, Filesystem};
use cargo::sources::SourceConfigMap;
use cargo::ops::{self, Packages, LibRule, FilterRule};

use crate::utils::{get_current_versions, parse_rustc_version};
use crate::error::Result;

use crate::Metadata;


/// Builds documentation of a crate and version.
///
/// Crate will be built into current working directory/crate-version.
///
/// It will build latest version, if no version is given.
// idea is to make cargo to download
// and build a crate and its documentation
// instead of doing it manually like in the previous version of cratesfyi
pub fn build_doc(name: &str, vers: Option<&str>, target: Option<&str>) -> Result<Package> {
    core::enable_nightly_features();
    let config = Config::default()?;
    let source_id = SourceId::crates_io(&config)?;

    let source_cfg_map = SourceConfigMap::new(&config)?;
    let mut source = source_cfg_map.load(source_id, &HashSet::new())?;

    let _lock = config.acquire_package_cache_lock()?;

    // update crates.io-index registry
    source.update()?;

    let dep = Dependency::parse_no_deprecated(name, vers, source_id)?;
    let deps = source.query_vec(&dep)?;
    let pkgid = deps.iter().map(|p| p.package_id()).max()
                     // FIXME: This is probably not a rusty way to handle options and results
                     //        or maybe it is who knows...
                     .ok_or(internal("no package id available"))?;

    let mut source_map = SourceMap::new();
    source_map.insert(source);

    let pkg_set = PackageSet::new(&[pkgid.clone()], source_map, &config)?;

    let pkg = pkg_set.get_one(pkgid)?.clone();

    let current_dir = env::current_dir()?;
    let target_dir = PathBuf::from(current_dir).join("cratesfyi");

    let metadata = Metadata::from_package(&pkg).map_err(|e| internal(e.to_string()))?;

    // This is only way to pass rustc_args to cargo.
    // CompileOptions::target_rustc_args is used only for the current crate,
    // and since docs.rs never runs rustc on the current crate, we assume rustc_args
    // will be used for the dependencies. That is why we are creating RUSTFLAGS environment
    // variable instead of using target_rustc_args.
    if let Some(rustc_args) = metadata.rustc_args {
        env::set_var("RUSTFLAGS", rustc_args.join(" "));
    }

    // since https://github.com/rust-lang/rust/pull/48511 we can pass --resource-suffix to
    // add correct version numbers to css and javascript files
    let mut rustdoc_args: Vec<String> =
        vec!["-Z".to_string(), "unstable-options".to_string(),
             "--resource-suffix".to_string(),
             format!("-{}", parse_rustc_version(get_current_versions()?.0)?),
             "--static-root-path".to_string(), "/".to_string(),
             "--disable-per-crate-search".to_string()];

    // since https://github.com/rust-lang/rust/pull/51384, we can pass --extern-html-root-url to
    // force rustdoc to link to other docs.rs docs for dependencies
    let source = source_cfg_map.load(source_id, &HashSet::new())?;
    for (name, dep) in resolve_deps(&pkg, &config, source)? {
        rustdoc_args.push("--extern-html-root-url".to_string());
        rustdoc_args.push(format!("{}=https://docs.rs/{}/{}",
                                  name.replace("-", "_"), dep.name(), dep.version()));
    }

    if let Some(package_rustdoc_args) = metadata.rustdoc_args {
        rustdoc_args.append(&mut package_rustdoc_args.iter().map(|s| s.to_owned()).collect());
    }

    let mut build_config = BuildConfig::new(&config,
                                                 None,
                                                 &target.map(|t| t.to_string()),
                                                 CompileMode::Doc { deps: false })?;
    build_config.release = false;
    build_config.message_format = MessageFormat::Human;

    let opts = ops::CompileOptions {
        config: &config,
        build_config,
        features: metadata.features.unwrap_or(Vec::new()),
        all_features: metadata.all_features,
        no_default_features: metadata.no_default_features,
        spec: Packages::Packages(Vec::new()),
        filter: ops::CompileFilter::new(LibRule::True,
                                        FilterRule::none(),
                                        FilterRule::none(),
                                        FilterRule::none(),
                                        FilterRule::none()),
        target_rustc_args: None,
        target_rustdoc_args: Some(rustdoc_args),
        local_rustdoc_args: None,
        export_dir: None,
    };

    let ws = Workspace::ephemeral(pkg, &config, Some(Filesystem::new(target_dir)), false)?;
    let exec: Arc<dyn Executor> = Arc::new(DefaultExecutor);
    ops::compile_ws(&ws, &opts, &exec)?;

    Ok(ws.current()?.clone())
}

fn resolve_deps<'cfg>(pkg: &Package, config: &'cfg Config, src: Box<dyn Source + 'cfg>)
    -> CargoResult<Vec<(String, Package)>>
{
    let mut registry = PackageRegistry::new(config)?;
    registry.add_preloaded(src);
    registry.lock_patches();

    let resolver = resolver::resolve(
        &[(pkg.summary().clone(), resolver::Method::Everything)],
        pkg.manifest().replace(),
        &mut registry,
        &Default::default(),
        None,
        false,
    )?;
    let dep_ids = resolver.deps(pkg.package_id()).map(|p| p.0).collect::<Vec<_>>();
    let pkg_set = registry.get(&dep_ids)?;
    let deps = pkg_set.get_many(dep_ids)?;

    let mut ret = Vec::new();
    for dep in deps {
        if let Some(d) = pkg.dependencies().iter().find(|d| d.package_name() == dep.name()) {
            ret.push((d.name_in_toml().to_string(), dep.clone()));
        }
    }

    Ok(ret)
}

/// Downloads a crate and returns Cargo Package.
pub fn get_package(name: &str, vers: Option<&str>) -> CargoResult<Package> {
    core::enable_nightly_features();
    debug!("Getting package with cargo");
    let config = Config::default()?;
    let source_id = SourceId::crates_io(&config)?;

    let source_map = SourceConfigMap::new(&config)?;
    let mut source = source_map.load(source_id, &HashSet::new())?;

    let _lock = config.acquire_package_cache_lock()?;

    source.update()?;

    let dep = Dependency::parse_no_deprecated(name, vers, source_id)?;
    let deps = source.query_vec(&dep)?;
    let pkgid = deps.iter().map(|p| p.package_id()).max()
                     // FIXME: This is probably not a rusty way to handle options and results
                     //        or maybe it is who knows...
                     .ok_or(internal("no package id available"))?;

    let mut source_map = SourceMap::new();
    source_map.insert(source);

    let pkg_set = PackageSet::new(&[pkgid.clone()], source_map, &config)?;

    let pkg = pkg_set.get_one(pkgid)?.clone();

    Ok(pkg)
}


/// Updates central crates-io.index repository
pub fn update_sources() -> CargoResult<()> {
    let config = Config::default()?;
    let source_id = SourceId::crates_io(&config)?;

    let _lock = config.acquire_package_cache_lock()?;

    let source_map = SourceConfigMap::new(&config)?;
    let mut source = source_map.load(source_id, &HashSet::new())?;

    source.update()
}


/// Gets source path of a downloaded package.
pub fn source_path(pkg: &Package) -> Option<&Path> {
    // parent of the manifest file is where source codes are stored
    pkg.manifest_path().parent()
}




#[cfg(test)]
mod test {
    use std::path::Path;
    use super::*;

    #[test]
    fn test_get_package() {
        let pkg = get_package("rand", None);
        assert!(pkg.is_ok());

        let pkg = pkg.unwrap();

        let manifest = pkg.manifest();
        assert_eq!(manifest.name().as_str(), "rand");
    }


    #[test]
    fn test_source_path() {
        let pkg = get_package("rand", None).unwrap();
        let source_path = source_path(&pkg).unwrap();
        assert!(source_path.is_dir());

        let cargo_toml_path = Path::new(source_path).join("Cargo.toml");
        assert!(cargo_toml_path.exists());
        assert!(cargo_toml_path.is_file());

        let src_path = Path::new(source_path).join("src");
        assert!(src_path.exists());
        assert!(src_path.is_dir());
    }
}
