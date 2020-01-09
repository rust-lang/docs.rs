use super::TestDatabase;
use crate::db::CratesIoData;
use crate::docbuilder::BuildResult;
use crate::utils::{Dependency, MetadataPackage, Target};
use failure::Error;

#[must_use = "FakeRelease does nothing until you call .create()"]
pub(crate) struct FakeRelease<'db> {
    db: &'db TestDatabase,
    package: MetadataPackage,
    build_result: BuildResult,
    /// name, content
    source_files: Vec<(String, Vec<u8>)>,
    /// name, content
    rustdoc_files: Vec<(String, Vec<u8>)>,
    doc_targets: Vec<String>,
    default_target: Option<String>,
    cratesio_data: CratesIoData,
    has_docs: bool,
    has_examples: bool,
}

impl<'db> FakeRelease<'db> {
    pub(super) fn new(db: &'db TestDatabase) -> Self {
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
            cratesio_data: CratesIoData {
                release_time: time::get_time(),
                yanked: false,
                downloads: 0,
                owners: Vec::new(),
            },
            has_docs: true,
            has_examples: false,
        }
    }

    pub(crate) fn name(mut self, new: &str) -> Self {
        self.package.name = new.into();
        self.package.id = format!("{}-id", new);
        self
    }

    pub(crate) fn version(mut self, new: &str) -> Self {
        self.package.version = new.into();
        self
    }

    pub(crate) fn build_result_successful(mut self, new: bool) -> Self {
        self.build_result.successful = new;
        self
    }

    pub(crate) fn cratesio_data_yanked(mut self, new: bool) -> Self {
        self.cratesio_data.yanked = new;
        self
    }

    pub(crate) fn file<M, P, D>(mut self, mimetype: M, path: P, data: D) -> Self
        where M: Into<String>,
              P: Into<String>,
              D: Into<Vec<u8>>,
        {
        let (mimetype, path, data) = (mimetype.into(), path.into(), data.into());
        self.files.push((mimetype, path, data));
        self
    }

    pub(crate) fn rustdoc_file<P, D>(mut self, path: P, data: D) -> Self
        where P: Into<String>,
              D: Into<Vec<u8>>,
        {
        let (path, data) = (path.into(), data.into());
        self.rustdoc_files.push((path, data));

        self
    }

    pub(crate) fn create(self) -> Result<i32, Error> {
        let tempdir = tempdir::TempDir::new("docs.rs-fake")?;

        let upload_files = |prefix: &str, files: Vec<(String, Vec<u8>)>, package: &MetadataPackage, db: &TestDatabase| {
            let path_prefix = tempdir.path().join(prefix);
            std::fs::create_dir(&path_prefix)?;

            for (path, data) in files {
                let file = path_prefix.join(&path);
                std::fs::write(file, data)?;
            }

            let prefix = format!("{}/{}/{}", prefix, package.name, package.version);
            crate::db::add_path_into_database(&db.conn(), &prefix, path_prefix)
        };

        let rustdoc_meta = upload_files("rustdoc", self.rustdoc_files, &self.package, self.db)?;
        log::debug!("added rustdoc files {}", rustdoc_meta);
        let source_meta = upload_files("source", self.source_files, &self.package, self.db)?;
        log::debug!("added source files {}", source_meta);

        let release_id = crate::db::add_package_into_database(
            &self.db.conn(),
            &self.package,
            tempdir.path(),
            &self.build_result,
            Some(source_meta),
            self.doc_targets,
            &self.default_target,
            &self.cratesio_data,
            self.has_docs,
            self.has_examples,
        )?;
        crate::db::add_build_into_database(&self.db.conn(), &release_id, &self.build_result)?;

        Ok(release_id)
    }
}
