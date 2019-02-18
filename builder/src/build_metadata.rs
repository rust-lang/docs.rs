//! Build DocrsPackageMetadata generator.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use log::debug;

use crate::build_result::BuildResult;
use crate::builder::source_path;
use crate::utils;
use crate::Result;
use cargo::core::Package;
use docsrs_package::metadata::{Dependencies, Dependency, DocsrsPackageMetadata};
use failure::{bail, err_msg};
use reqwest;
use semver;
use serde_json::{self, Value};
use walkdir::WalkDir;
use zip::write::FileOptions;
use zip::CompressionMethod::Deflated;
use zip::ZipWriter;

pub struct BuildMetadata(DocsrsPackageMetadata);

impl BuildMetadata {
    pub fn new<'a>(result: BuildResult<'a>, doc_targets: Vec<String>) -> Result<BuildMetadata> {
        Ok(BuildMetadata(DocsrsPackageMetadata {
            name: result.pkg().manifest().name().as_str().to_string(),
            version: format!("{}", result.pkg().manifest().version()),
            description: result.pkg().manifest().metadata().description.clone(),
            target_name: result.pkg().targets()[0].name().replace("-", "_"),
            release_time: get_release_time(result.pkg())?,
            dependencies: convert_dependencies(result.pkg()),
            build_status: result.build_status(),
            rustdoc_status: result.have_docs(),
            license: result.pkg().manifest().metadata().license.clone(),
            repository: result.pkg().manifest().metadata().repository.clone(),
            homepage: result.pkg().manifest().metadata().homepage.clone(),
            documentation: result.pkg().manifest().metadata().documentation.clone(),
            rustdoc_content: read_rustdoc(result.pkg())?,
            readme_content: read_readme(result.pkg())?,
            authors: result.pkg().manifest().metadata().authors.clone(),
            keywords: result.pkg().manifest().metadata().keywords.clone(),
            categories: result.pkg().manifest().metadata().categories.clone(),
            have_examples: result.have_examples()?,
            doc_targets,
            is_library: result.is_library(),
            default_target: result
                .metadata()
                .default_target
                .as_ref()
                .map(|t| t.to_string())
                .unwrap_or_else(|| crate::DEFAULT_TARGET.to_string()),
            rustc_version: utils::get_rustc_version()?,
            builder_version: format!("docsrs-builder {}", env!("CARGO_PKG_VERSION")),
        }))
    }

    pub fn create_package(&self, target_dir: impl AsRef<Path>, pkg: &Package) -> Result<()> {
        // create target directory
        fs::create_dir_all(&target_dir)?;

        // save build_metadata as docsrs.json into target directory
        self.save_into(&target_dir)?;

        // copy sources into target directory
        self.copy_sources(&target_dir, &pkg)?;

        // create a zip archive of documentation package
        self.create_zip(&target_dir)?;

        Ok(())
    }

    /// Saves metadata into a directory. Path must be a directory.
    fn save_into(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = Path::new(path.as_ref()).join("docsrs.json");
        debug!("Writing package metadata into: {:?}", path);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        fs::write(path, serde_json::to_string_pretty(&self.0)?.as_bytes())?;
        Ok(())
    }

    fn copy_sources(&self, path: impl AsRef<Path>, pkg: &Package) -> Result<()> {
        let source_path = source_path(pkg).ok_or_else(|| err_msg("Source path not available"))?;
        let target_dir = Path::new(path.as_ref())
            .join("source")
            .join(&self.0.name)
            .join(&self.0.version);

        debug!("Copying sources from {:?} to {:?}", source_path, target_dir);

        for entry in walkdir::WalkDir::new(source_path) {
            let entry = entry?;
            let entry_path = entry.path();
            let destination_path =
                Path::new(&target_dir).join(entry_path.strip_prefix(source_path)?);
            if entry_path.is_dir() {
                fs::create_dir_all(destination_path)?;
            } else {
                fs::copy(entry_path, destination_path)?;
            }
        }

        Ok(())
    }

    /// Creates a zip archive of package
    fn create_zip(&self, path: impl AsRef<Path>) -> Result<()> {
        let zip_file_name = format!("{}-{}.zip", self.0.name, self.0.version);
        let zip_file_path = Path::new(path.as_ref()).join(zip_file_name);
        debug!("Creating zip archive: {:?}", zip_file_path);

        // remove old zip file if its exists
        if zip_file_path.exists() {
            fs::remove_file(&zip_file_path)?;
        }

        let zip_file = File::create(&zip_file_path)?;
        let mut zip = ZipWriter::new(zip_file);

        // add docsrs.json to zip
        self.add_into_zip(
            &mut zip,
            &path,
            Path::new(path.as_ref()).join("docsrs.json"),
        )?;

        // add crate source into zip
        self.add_into_zip(
            &mut zip,
            &path,
            Path::new(path.as_ref())
                .join("source")
                .join(&self.0.name)
                .join(&self.0.version),
        )?;

        // only add documentation if crate successfully built and have documentation
        if self.0.rustdoc_status {
            // add default doc directory
            self.add_into_zip(
                &mut zip,
                &path,
                Path::new(path.as_ref())
                    .join(self.0.default_target.as_str())
                    .join("doc"),
            )?;

            // add successfully targets to zip
            for successfully_target in &self.0.doc_targets {
                self.add_into_zip(
                    &mut zip,
                    &path,
                    Path::new(path.as_ref())
                        .join(successfully_target)
                        .join("doc"),
                )?;
            }
        }

        zip.finish()?;
        Ok(())
    }

    fn add_into_zip(
        &self,
        mut zip: &mut ZipWriter<File>,
        work_dir: impl AsRef<Path>,
        path: impl AsRef<Path>,
    ) -> Result<()> {
        if path.as_ref().is_file() {
            let options = FileOptions::default().compression_method(Deflated);
            let name = path
                .as_ref()
                .strip_prefix(Path::new(work_dir.as_ref()))?
                .to_str()
                .ok_or_else(|| err_msg("Cannot convert entry path to str"))?;
            debug!("Adding {:?} into zip", name);
            let content = fs::read(&path)?;
            zip.start_file(name, options)?;
            zip.write_all(&content)?;
            return Ok(());
        }

        for entry in WalkDir::new(path.as_ref()) {
            let entry = entry?;
            let entry_path = entry.path();
            if entry_path.is_file() {
                self.add_into_zip(&mut zip, work_dir.as_ref(), entry_path)?;
            }
        }

        Ok(())
    }
}

fn get_release_time(pkg: &Package) -> Result<String> {
    let url = format!(
        "https://crates.io/api/v1/crates/{}/versions",
        pkg.manifest().name()
    );
    let json: Value = serde_json::from_str(&reqwest::get(&url)?.text()?)?;

    for version in json
        .as_object()
        .and_then(|o| o.get("versions"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| err_msg("Versions does not a JSON object"))?
    {
        let version = version
            .as_object()
            .ok_or_else(|| err_msg("Version does not a JSON object"))?;
        let version_num = version
            .get("num")
            .and_then(|v| v.as_str())
            .ok_or_else(|| err_msg("Num does not a JSON string"))?;

        if &semver::Version::parse(version_num).unwrap() == pkg.manifest().version() {
            return Ok(version
                .get("created_at")
                .and_then(|c| c.as_str())
                .map(str::to_string)
                .ok_or_else(|| err_msg("created_at does not a JSON string"))?);
        }
    }

    bail!("Unable to get version")
}

fn convert_dependencies(pkg: &Package) -> Dependencies {
    let mut dependencies = Dependencies::default();

    for dependency in pkg.manifest().dependencies() {
        let dependency_meta = Dependency {
            name: dependency.package_name().to_string(),
            version: format!("{}", dependency.version_req()),
            optional: dependency.is_optional(),
        };
        use cargo::core::dependency::Kind;
        match dependency.kind() {
            Kind::Normal => dependencies.normal.push(dependency_meta),
            Kind::Development => dependencies.development.push(dependency_meta),
            Kind::Build => dependencies.build.push(dependency_meta),
        };
    }
    dependencies
}

/// Reads rustdoc from root of the crate (usually src/lib.rs).
fn read_rustdoc(pkg: &Package) -> Result<Option<String>> {
    let src_path = pkg.manifest().targets()[0]
        .src_path()
        .path()
        .ok_or_else(|| err_msg("Source path not available"))?;

    let reader = File::open(src_path).map(BufReader::new)?;
    let mut rustdoc = String::new();

    for line in reader.lines() {
        let line = line?;
        if line.starts_with("//!") {
            // some lines may or may not have a space between the `//!` and the start of the text
            let line = line.trim_start_matches("//!").trim_start();
            if !line.is_empty() {
                rustdoc.push_str(line);
            }
            rustdoc.push('\n');
        }
    }

    if rustdoc.is_empty() {
        Ok(None)
    } else {
        Ok(Some(rustdoc))
    }
}

/// Reads readme if there is any read defined in Cargo.toml of a Package
fn read_readme(pkg: &Package) -> Result<Option<String>> {
    let readme_path = Path::new(source_path(&pkg).ok_or_else(|| err_msg("File not found"))?).join(
        pkg.manifest()
            .metadata()
            .readme
            .clone()
            .unwrap_or_else(|| "README.md".to_owned()),
    );

    if !readme_path.exists() {
        return Ok(None);
    }

    Ok(Some(fs::read_to_string(readme_path)?))
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::builder::Builder;

    #[test]
    fn test_get_release_time() {
        let builder = Builder::new("rand", Some("=0.6.5"), "./").unwrap();
        let pkg = builder.get_package().unwrap();
        let release_time = get_release_time(&pkg).unwrap();
        assert_eq!(release_time, "2019-01-28T09:56:57.788327+00:00");
    }

    #[test]
    fn test_convert_dependencies() {
        let builder = Builder::new("rand", Some("=0.6.5"), "./").unwrap();
        let pkg = builder.get_package().unwrap();
        let dependencies = convert_dependencies(&pkg);

        let normal_dependencies = vec![
            "log",
            "packed_simd",
            "rand_chacha",
            "rand_core",
            "rand_hc",
            "rand_isaac",
            "rand_jitter",
            "rand_os",
            "rand_pcg",
            "rand_xorshift",
            "libc",
            "winapi",
        ];
        let development_dependencies = vec!["average", "rand_xoshiro"];
        let build_dependencies = vec!["autocfg"];

        for dependency in normal_dependencies {
            assert!(dependencies
                .normal
                .iter()
                .filter(|d| d.name == dependency)
                .next()
                .is_some());
        }

        for dependency in development_dependencies {
            assert!(dependencies
                .development
                .iter()
                .filter(|d| d.name == dependency)
                .next()
                .is_some());
        }

        for dependency in build_dependencies {
            assert!(dependencies
                .build
                .iter()
                .filter(|d| d.name == dependency)
                .next()
                .is_some());
        }
    }

    #[test]
    fn test_read_rustdoc() {
        let builder = Builder::new("rand", Some("=0.6.5"), "./").unwrap();
        let pkg = builder.get_package().unwrap();
        let rustdoc = read_rustdoc(&pkg).unwrap().unwrap();
        let rand_rustdoc = "Utilities for random number generation\n\nRand provides utilities \
                            to generate random numbers, to convert them to\nuseful types and \
                            distributions, and some randomness-related algorithms.\n\n# Quick \
                            Start\n\nTo get you started quickly, the easiest and highest-level \
                            way to get\na random value is to use [`random()`]; alternatively \
                            you can use\n[`thread_rng()`]. The [`Rng`] trait provides a useful \
                            API on all RNGs, while\nthe [`distributions`] and [`seq`] modules \
                            provide further\nfunctionality on top of RNGs.\n\n```\n\
                            use rand::prelude::*;\n\nif rand::random() { // generates \
                            a boolean\n// Try printing a random unicode code point (probably a \
                            bad idea)!\nprintln!(\"char: {}\", rand::random::<char>());\n}\n\n\
                            let mut rng = rand::thread_rng();\nlet y: f64 = rng.gen(); // \
                            generates a float between 0 and 1\n\nlet mut nums: Vec<i32> = \
                            (1..100).collect();\nnums.shuffle(&mut rng);\n```\n\n# The Book\n\n\
                            For the user guide and futher documentation, please read\n\
                            [The Rust Rand Book](https://rust-random.github.io/book).\n";
        assert_eq!(rustdoc, rand_rustdoc.to_string());
    }

    #[test]
    fn test_read_readme() {
        let builder = Builder::new("rand", Some("=0.6.5"), "./").unwrap();
        let pkg = builder.get_package().unwrap();
        let readme = read_readme(&pkg).unwrap().unwrap();
        assert!(!readme.is_empty());
    }
}
