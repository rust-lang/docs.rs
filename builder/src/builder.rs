use std::collections::HashSet;
use std::env;
use std::fs::remove_dir_all;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use log::{debug, info};

use failure::{bail, err_msg};

use cargo::core::compiler::{BuildConfig, CompileMode, DefaultExecutor, Executor, MessageFormat};
use cargo::core::package::PackageSet;
use cargo::core::registry::PackageRegistry;
use cargo::core::resolver;
use cargo::core::source::SourceMap;
use cargo::core::{self, Dependency, Package, Source, SourceId, Workspace};
use cargo::ops::{self, Packages};
use cargo::sources::SourceConfigMap;
use cargo::util::{CargoResult, Config, Filesystem};

use crate::build_result::BuildResult;
use crate::package_metadata::PackageMetadata;
use crate::utils::resource_suffix;

use crate::Result;

pub struct Builder {
    name: String,
    version: Option<String>,
    target_dir: PathBuf,

    // cargo fields
    config: Config,
    source_id: SourceId,
}

impl Builder {
    pub fn new(name: &str, version: Option<&str>, target_dir: impl AsRef<Path>) -> Result<Builder> {
        core::enable_nightly_features();
        let config = Config::default()?;
        let source_id = SourceId::crates_io(&config)?;

        Ok(Builder {
            name: name.to_string(),
            version: version.map(|v| v.to_string()),
            target_dir: target_dir.as_ref().to_path_buf(),
            config,
            source_id,
        })
    }

    pub fn get_package(&self) -> Result<Package> {
        let source_map = SourceConfigMap::new(&self.config)?;
        let mut source = source_map.load(self.source_id, &HashSet::new())?;

        source.update()?;

        let dep = Dependency::parse_no_deprecated(
            &self.name,
            self.version.as_ref().map(String::as_str),
            self.source_id,
        )?;
        let deps = source.query_vec(&dep)?;
        let pkgid = deps
            .iter()
            .map(|p| p.package_id())
            .max()
            .ok_or_else(|| err_msg("no package id available"))?;

        let mut source_map = SourceMap::new();
        source_map.insert(source);

        let pkg_set = PackageSet::new(&[pkgid], source_map, &self.config)?;
        let pkg = pkg_set.get_one(pkgid)?.clone();

        Ok(pkg)
    }

    /// Remove documentation directories in target directory.
    ///
    /// This function will only remove "doc" directories and keep the rest (debug) for cache.
    pub fn clean(&self) -> Result<()> {
        // remove default documentation directory
        let default_doc_dir = Path::new(&self.target_dir).join("doc");
        if default_doc_dir.exists() {
            remove_dir_all(default_doc_dir)?;
        }

        // remove target documentation directories
        for target in crate::TARGETS.iter() {
            let target_doc_dir = Path::new(&self.target_dir).join(target).join("doc");
            if target_doc_dir.exists() {
                remove_dir_all(target_doc_dir)?;
            }
        }

        Ok(())
    }

    pub fn get_metadata(&self, pkg: &Package) -> Result<PackageMetadata> {
        Ok(PackageMetadata::from_package(pkg)?)
    }

    /// Installs system dependencies if we are running in a docsrs builder instance
    ///
    /// Checks DOCSRS_BUILD environment variable before trying to install dependencies.
    pub fn install_system_dependencies(&self, metadata: &PackageMetadata) -> Result<()> {
        // skip installing system dependencies if this library is not running
        // in a docs.rs build environment
        if env::var("DOCSRS_BUILD_ENV").is_err() || metadata.dependencies.is_none() {
            return Ok(());
        }

        info!("Installing system dependencies");

        use std::process::{Command, Stdio};
        Command::new("apt-get")
            .arg("update")
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()?;
        Command::new("apt-get")
            .args(&["install", "-y"])
            .args(
                metadata
                    .dependencies
                    .as_ref()
                    .expect("This will never fail"),
            )
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()?;

        Ok(())
    }

    pub fn build_doc<'a>(
        &'a self,
        pkg: &'a Package,
        metadata: &'a PackageMetadata,
        target: &'a str,
    ) -> BuildResult<'a> {
        info!(
            "Building {} {} for {}",
            self.name,
            self.version.as_ref().map(String::as_str).unwrap_or(""),
            target
        );

        let build_res = self.cargo_doc(pkg, metadata, target);

        if build_res.is_err() {
            debug!("{:#?}", build_res);
        }

        BuildResult::new(pkg, metadata, target, &self.target_dir, build_res.is_ok())
    }

    fn cargo_doc(&self, pkg: &Package, metadata: &PackageMetadata, target: &str) -> Result<()> {
        let source_id = SourceId::crates_io(&self.config)?;
        let source_cfg_map = SourceConfigMap::new(&self.config)?;

        // Make sure target is in our supported targets
        if crate::TARGETS.iter().find(|&&t| t == target).is_none() {
            bail!("Target is not supported by docs.rs");
        }

        // This is only way to pass rustc_args to cargo.
        // CompileOptions::target_rustc_args is used only for the current crate,
        // and since docs.rs never runs rustc on the current crate, we assume rustc_args
        // will be used for the dependencies. This is why we are creating RUSTFLAGS environment
        // variable instead of using target_rustc_args.
        if let Some(rustc_args) = metadata.rustc_args.as_ref() {
            env::set_var("RUSTFLAGS", rustc_args.join(" "));
        }

        // since https://github.com/rust-lang/rust/pull/48511 we can pass --resource-suffix to
        // add correct version numbers to css and javascript files
        let mut rustdoc_args = vec![
            "-Z".to_string(),
            "unstable-options".to_string(),
            "--resource-suffix".to_string(),
            resource_suffix()?,
            "--static-root-path".to_string(),
            "/".to_string(),
            "--disable-per-crate-search".to_string(),
        ];

        // since https://github.com/rust-lang/rust/pull/51384,
        // we can pass --extern-html-root-url to force rustdoc to
        // link to other docs.rs docs for dependencies
        let source = source_cfg_map.load(source_id, &HashSet::new())?;
        for (name, dep) in resolve_deps(&pkg, &self.config, source)? {
            rustdoc_args.push("--extern-html-root-url".to_string());
            rustdoc_args.push(format!(
                "{}=https://docs.rs/{}/{}",
                name.replace("-", "_"),
                dep.name(),
                dep.version()
            ));
        }

        // append rustdoc_args from [package.metadata.docs.rs]
        if let Some(package_rustdoc_args) = metadata.rustdoc_args.as_ref() {
            rustdoc_args.append(&mut package_rustdoc_args.iter().map(|s| s.to_string()).collect());
        }

        let mut build_config = BuildConfig::new(
            &self.config,
            None,
            &Some(target.to_string()),
            CompileMode::Doc { deps: false },
        )?;
        build_config.release = false;
        build_config.message_format = MessageFormat::Human;

        let opts = ops::CompileOptions {
            config: &self.config,
            build_config,
            features: metadata.features.clone().unwrap_or_default(),
            all_features: metadata.all_features,
            no_default_features: metadata.no_default_features,
            spec: Packages::Packages(Vec::new()),
            filter: ops::CompileFilter::new(
                true,
                Vec::new(),
                false,
                Vec::new(),
                false,
                Vec::new(),
                false,
                Vec::new(),
                false,
                false,
            ),
            target_rustc_args: None,
            target_rustdoc_args: Some(rustdoc_args),
            local_rustdoc_args: None,
            export_dir: None,
        };

        let ws = Workspace::ephemeral(
            pkg.clone(),
            &self.config,
            Some(Filesystem::new(self.target_dir.clone())),
            false,
        )?;
        let exec: Arc<Executor> = Arc::new(DefaultExecutor);
        let source = source_cfg_map.load(source_id, &HashSet::new())?;
        ops::compile_ws(&ws, Some(source), &opts, &exec)?;

        Ok(())
    }
}

fn resolve_deps<'cfg>(
    pkg: &Package,
    config: &'cfg Config,
    src: Box<Source + 'cfg>,
) -> CargoResult<Vec<(String, Package)>> {
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
    let dep_ids = resolver
        .deps(pkg.package_id())
        .map(|p| p.0)
        .collect::<Vec<_>>();
    let pkg_set = registry.get(&dep_ids)?;
    let deps = pkg_set.get_many(dep_ids)?;

    let mut ret = Vec::new();
    for dep in deps {
        if let Some(d) = pkg
            .dependencies()
            .iter()
            .find(|d| d.package_name() == dep.name())
        {
            ret.push((d.name_in_toml().to_string(), dep.clone()));
        }
    }

    Ok(ret)
}

/// Gets source path of a downloaded package.
pub fn source_path(pkg: &Package) -> Option<&Path> {
    // parent of the manifest file is where source codes are stored
    pkg.manifest_path().parent()
}

#[cfg(test)]
mod test {
    use super::*;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn test_get_package() {
        let builder = Builder::new("rand", Some("0.6"), "./").unwrap();
        let pkg = builder.get_package();
        assert!(pkg.is_ok());

        let pkg = pkg.unwrap();

        let manifest = pkg.manifest();
        assert_eq!(manifest.name().as_str(), "rand");
    }

    #[test]
    fn test_build_doc() {
        let target_dir = tempdir().unwrap();
        let builder = Builder::new("acme-client", Some("0.0.0"), &target_dir).unwrap();
        let pkg = builder.get_package().unwrap();
        let metadata = builder.get_metadata(&pkg).unwrap();
        let build_res = builder.build_doc(&pkg, &metadata, crate::DEFAULT_TARGET);

        assert!(target_dir
            .path()
            .to_owned()
            .join(crate::DEFAULT_TARGET)
            .join("doc")
            .exists());
        assert!(build_res.have_docs());
        assert!(!build_res.have_examples().unwrap());
    }

    #[test]
    fn test_source_path() {
        let builder = Builder::new("rand", Some("0.6"), "./").unwrap();
        let pkg = builder.get_package().unwrap();
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
