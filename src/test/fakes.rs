use super::TestDatabase;
use crate::db::CratesIoData;
use crate::docbuilder::BuildResult;
use crate::utils::{Dependency, MetadataPackage, Target};
use failure::Error;
use rustc_serialize::json::Json;

#[must_use = "FakeRelease does nothing until you call .create()"]
pub(crate) struct FakeRelease<'db> {
    db: &'db TestDatabase,
    package: MetadataPackage,
    build_result: BuildResult,
    files: Vec<(String, String, Vec<u8>)>,
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
            files: Vec::new(),
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

    pub(crate) fn create(self) -> Result<i32, Error> {
        let tempdir = tempdir::TempDir::new("docs.rs-fake")?;

        let files: Vec<_> = self.files.into_iter().map(|(mimetype, path, data)| {
            let file = tempdir.path().join(&path);
            std::fs::write(file, data)?;

            Ok(Json::Array(vec![Json::String(mimetype), Json::String(path)]))
        }).collect::<Result<_, Error>>()?;

        let release_id = crate::db::add_package_into_database(
            &self.db.conn(),
            &self.package,
            tempdir.path(),
            &self.build_result,
            if files.is_empty() { None } else { Some(Json::Array(files)) },
            self.doc_targets,
            &self.default_target,
            &self.cratesio_data,
            self.has_docs,
            self.has_examples,
        )?;
        crate::db::add_build_into_database(&self.db.conn(), &release_id, &self.build_result)?;

        let prefix = format!("rustdoc/{}/{}", self.package.name, self.package.version);
        let added_files = crate::db::add_path_into_database(&self.db.conn(), &prefix, tempdir.path())?;
        log::debug!("added files {}", added_files);

        Ok(release_id)
    }
}
