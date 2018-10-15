//! Crate documentation builder
//!
//! This module is extremely similar to cargo install operation, except it's building
//! documentation of a crate and not installing anything.

use std::path::{Path, PathBuf};
use std::env;
use std::sync::Arc;

use cargo::core::{SourceId, Dependency, Source, Package, Workspace};
use cargo::core::compiler::{DefaultExecutor, CompileMode, MessageFormat, BuildConfig, Executor};
use cargo::util::{CargoResult, Config, internal, Filesystem};
use cargo::sources::SourceConfigMap;
use cargo::ops::{self, Packages};

use utils::{get_current_versions, parse_rustc_version};
use error::Result;

use Metadata;


/// Builds documentation of a crate and version.
///
/// Crate will be built into current working directory/crate-version.
///
/// It will build latest version, if no version is given.
// idea is to make cargo to download
// and build a crate and its documentation
// instead of doing it manually like in the previous version of cratesfyi
pub fn build_doc(name: &str, vers: Option<&str>, target: Option<&str>) -> Result<Package> {
    let config = try!(Config::default());
    let source_id = try!(SourceId::crates_io(&config));

    let source_map = try!(SourceConfigMap::new(&config));
    let mut source = try!(source_map.load(&source_id));

    // update crates.io-index registry
    try!(source.update());

    let dep = try!(Dependency::parse_no_deprecated(name, vers, &source_id));
    let deps = try!(source.query_vec(&dep));
    let pkg = try!(deps.iter().map(|p| p.package_id()).max()
                   // FIXME: This is probably not a rusty way to handle options and results
                   //        or maybe it is who knows...
                   .map(|pkgid| source.download(pkgid))
                   .unwrap_or(Err(internal("PKG download error"))));

    let current_dir = try!(env::current_dir());
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
    // TODO: we can add --extern-html-root-url too, thanks to
    // https://github.com/rust-lang/rust/pull/51384
    let mut rustdoc_args: Vec<String> =
        vec!["-Z".to_string(), "unstable-options".to_string(),
             "--resource-suffix".to_string(),
             format!("-{}", parse_rustc_version(get_current_versions()?.0)?)];
    if let Some(package_rustdoc_args) = metadata.rustdoc_args {
        rustdoc_args.append(&mut package_rustdoc_args.iter().map(|s| s.to_owned()).collect());
    }

    let mut build_config = try!(BuildConfig::new(&config,
                                                 None,
                                                 &target.map(|t| t.to_string()),
                                                 CompileMode::Doc { deps: false }));
    build_config.release = false;
    build_config.message_format = MessageFormat::Human;

    let opts = ops::CompileOptions {
        config: &config,
        build_config,
        features: metadata.features.unwrap_or(Vec::new()),
        all_features: metadata.all_features,
        no_default_features: metadata.no_default_features,
        spec: Packages::Packages(Vec::new()),
        filter: ops::CompileFilter::new(true,
                                        Vec::new(), false,
                                        Vec::new(), false,
                                        Vec::new(), false,
                                        Vec::new(), false,
                                        false),
        target_rustc_args: None,
        target_rustdoc_args: Some(rustdoc_args),
        export_dir: None,
    };

    let ws = try!(Workspace::ephemeral(pkg, &config, Some(Filesystem::new(target_dir)), false));
    let exec: Arc<Executor> = Arc::new(DefaultExecutor);
    try!(ops::compile_ws(&ws, Some(source), &opts, &exec));

    Ok(try!(ws.current()).clone())
}



/// Downloads a crate and returns Cargo Package.
pub fn get_package(name: &str, vers: Option<&str>) -> CargoResult<Package> {
    debug!("Getting package with cargo");
    let config = try!(Config::default());
    let source_id = try!(SourceId::crates_io(&config));

    let source_map = try!(SourceConfigMap::new(&config));
    let mut source = try!(source_map.load(&source_id));

    try!(source.update());

    let dep = try!(Dependency::parse_no_deprecated(name, vers, &source_id));
    let deps = try!(source.query_vec(&dep));
    let pkg = try!(deps.iter().map(|p| p.package_id()).max()
                   // FIXME: This is probably not a rusty way to handle options and results
                   //        or maybe it is who knows...
                   .map(|pkgid| source.download(pkgid))
                   .unwrap_or(Err(internal("PKG download error"))));

    Ok(pkg)
}


/// Updates central crates-io.index repository
pub fn update_sources() -> CargoResult<()> {
    let config = try!(Config::default());
    let source_id = try!(SourceId::crates_io(&config));

    let source_map = try!(SourceConfigMap::new(&config));
    let mut source = try!(source_map.load(&source_id));

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
