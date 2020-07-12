use super::TestDatabase;
use crate::docbuilder::BuildResult;
use crate::index::api::RegistryCrateData;
use crate::utils::{Dependency, MetadataPackage, Target};
use chrono::{DateTime, Utc};
use failure::Error;

#[must_use = "FakeRelease does nothing until you call .create()"]
pub(crate) struct FakeRelease<'a> {
    db: &'a TestDatabase,
    package: MetadataPackage,
    build_result: BuildResult,
    /// name, content
    source_files: Vec<(&'a str, &'a [u8])>,
    /// name, content
    rustdoc_files: Vec<(&'a str, &'a [u8])>,
    doc_targets: Vec<String>,
    default_target: Option<&'a str>,
    registry_crate_data: RegistryCrateData,
    has_docs: bool,
    has_examples: bool,
}

impl<'a> FakeRelease<'a> {
    pub(super) fn new(db: &'a TestDatabase) -> Self {
        FakeRelease {
            db,
            package: MetadataPackage {
                id: "fake-package-id".into(),
                name: "fake-package".into(),
                version: "1.0.0".into(),
                license: Some("MIT".into()),
                repository: Some("https://git.example.com".into()),
                homepage: Some("https://www.example.com".into()),
                description: Some("Fake package".into()),
                documentation: Some("https://docs.example.com".into()),
                dependencies: vec![Dependency {
                    name: "fake-dependency".into(),
                    req: "^1.0.0".into(),
                    kind: None,
                }],
                targets: vec![Target::dummy_lib("fake_package".into(), None)],
                readme: None,
                keywords: vec!["fake".into(), "package".into()],
                authors: vec!["Fake Person <fake@example.com>".into()],
            },
            build_result: BuildResult {
                rustc_version: "rustc 2.0.0-nightly (000000000 1970-01-01)".into(),
                docsrs_version: "docs.rs 1.0.0 (000000000 1970-01-01)".into(),
                build_log: "It works!".into(),
                successful: true,
            },
            source_files: Vec::new(),
            rustdoc_files: Vec::new(),
            doc_targets: Vec::new(),
            default_target: None,
            registry_crate_data: RegistryCrateData {
                release_time: Utc::now(),
                yanked: false,
                downloads: 0,
                owners: Vec::new(),
            },
            has_docs: true,
            has_examples: false,
        }
    }

    pub(crate) fn downloads(mut self, downloads: i32) -> Self {
        self.registry_crate_data.downloads = downloads;
        self
    }

    pub(crate) fn description(mut self, new: impl Into<String>) -> Self {
        self.package.description = Some(new.into());
        self
    }

    pub(crate) fn release_time(mut self, new: DateTime<Utc>) -> Self {
        self.registry_crate_data.release_time = new;
        self
    }

    pub(crate) fn name(mut self, new: &str) -> Self {
        self.package.name = new.into();
        self.package.id = format!("{}-id", new);
        self.package.targets[0].name = new.into();
        self
    }

    pub(crate) fn version(mut self, new: &str) -> Self {
        self.package.version = new.into();
        self
    }

    pub fn author(mut self, author: &str) -> Self {
        self.package.authors = vec![author.into()];
        self
    }

    pub(crate) fn repo(mut self, repo: impl Into<String>) -> Self {
        self.package.repository = Some(repo.into());
        self
    }

    pub(crate) fn build_result_successful(mut self, new: bool) -> Self {
        self.has_docs = new;
        self.build_result.successful = new;
        self
    }

    pub(crate) fn yanked(mut self, new: bool) -> Self {
        self.registry_crate_data.yanked = new;
        self
    }

    pub(crate) fn rustdoc_file(mut self, path: &'a str, data: &'a [u8]) -> Self {
        self.rustdoc_files.push((path, data));
        self
    }

    pub(crate) fn source_file(mut self, path: &'a str, data: &'a [u8]) -> Self {
        self.source_files.push((path, data));
        self
    }

    pub(crate) fn default_target(mut self, target: &'a str) -> Self {
        self = self.add_target(target);
        self.default_target = Some(target);
        self
    }

    pub(crate) fn add_target(mut self, target: &str) -> Self {
        self.doc_targets.push(target.into());
        self
    }

    pub(crate) fn binary(mut self, bin: bool) -> Self {
        self.has_docs = !bin;
        if bin {
            for target in self.package.targets.iter_mut() {
                target.crate_types = vec!["bin".into()];
            }
        }
        self
    }

    pub(crate) fn add_platform<S: Into<String>>(mut self, platform: S) -> Self {
        let platform = platform.into();
        let name = self.package.targets[0].name.clone();
        let target = Target::dummy_lib(name, Some(platform.clone()));
        self.package.targets.push(target);
        self.doc_targets.push(platform);
        self
    }

    /// Returns the release_id
    pub(crate) fn create(self) -> Result<i32, Error> {
        use std::collections::HashSet;
        use std::fs;
        use std::path::Path;

        let tempdir = tempfile::Builder::new().prefix("docs.rs-fake").tempdir()?;
        let package = self.package;
        let db = self.db;

        let mut source_meta = None;
        let mut algs = HashSet::new();
        if self.build_result.successful {
            let upload_files = |prefix: &str, files: &[(&str, &[u8])], target: Option<&str>| {
                let mut path_prefix = tempdir.path().join(prefix);
                if let Some(target) = target {
                    path_prefix.push(target);
                }
                fs::create_dir(&path_prefix)?;

                for (path, data) in files {
                    // allow `src/main.rs`
                    if let Some(parent) = Path::new(path).parent() {
                        fs::create_dir_all(path_prefix.join(parent))?;
                    }
                    let file = path_prefix.join(&path);
                    log::debug!("writing file {}", file.display());
                    fs::write(file, data)?;
                }

                let prefix = format!(
                    "{}/{}/{}/{}",
                    prefix,
                    package.name,
                    package.version,
                    target.unwrap_or("")
                );
                log::debug!("adding directory {} from {}", prefix, path_prefix.display());
                crate::db::add_path_into_database(&db.conn(), &prefix, path_prefix)
            };

            let index = [&package.name, "index.html"].join("/");
            let mut rustdoc_files = self.rustdoc_files;
            if package.is_library() && !rustdoc_files.iter().any(|(path, _)| path == &index) {
                rustdoc_files.push((&index, b"default index content"));
            }
            for (source_path, data) in &self.source_files {
                if source_path.starts_with("src/") {
                    let updated = ["src", &package.name, &source_path[4..]].join("/");
                    rustdoc_files.push((Box::leak(Box::new(updated)), data));
                }
            }
            let (rustdoc_meta, new_algs) = upload_files("rustdoc", &rustdoc_files, None)?;
            algs.extend(new_algs);
            log::debug!("added rustdoc files {}", rustdoc_meta);
            match upload_files("source", &self.source_files, None)? {
                (json, new_algs) => {
                    source_meta = Some(json);
                    algs.extend(new_algs);
                }
            }
            log::debug!("added source files {}", source_meta.as_ref().unwrap());

            for target in &package.targets[1..] {
                let platform = target.src_path.as_ref().unwrap();
                upload_files("rustdoc", &rustdoc_files, Some(platform))?;
                log::debug!("added platform files for {}", platform);
            }
        }

        let release_id = crate::db::add_package_into_database(
            &db.conn(),
            &package,
            tempdir.path(),
            &self.build_result,
            self.default_target.unwrap_or("x86_64-unknown-linux-gnu"),
            source_meta,
            self.doc_targets,
            &self.registry_crate_data,
            self.has_docs,
            self.has_examples,
            HashSet::new(),
        )?;
        crate::db::add_build_into_database(&db.conn(), release_id, &self.build_result)?;

        Ok(release_id)
    }
}
