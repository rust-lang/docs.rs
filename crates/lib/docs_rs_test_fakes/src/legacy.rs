use anyhow::{Context as _, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as b64};
use chrono::{DateTime, Utc};
use docs_rs_cargo_metadata::{Dependency, MetadataPackage, Target};
use docs_rs_database::{
    Pool,
    releases::{initialize_build, initialize_crate, initialize_release, update_build_status},
};
use docs_rs_registry_api::{CrateData, CrateOwner, ReleaseData};
use docs_rs_rustdoc_json::{RUSTDOC_JSON_COMPRESSION_ALGORITHMS, RustdocJsonFormatVersion};
use docs_rs_storage::{
    AsyncStorage, FileEntry, compress, file_list_to_json, rustdoc_archive_path, rustdoc_json_path,
    source_archive_path,
};
use docs_rs_types::{
    BuildId, BuildStatus, CompressionAlgorithm, DocCoverage, KrateName, ReleaseId, Version,
    VersionReq,
};
use std::{
    collections::{BTreeMap, HashMap},
    fmt, iter,
    sync::Arc,
};
use tracing::debug;

/// Create a fake release in the database that failed before the build.
/// This is a temporary small factory function only until we refactored the
/// `FakeRelease` and `FakeBuild` factories to be more flexible.
pub async fn fake_release_that_failed_before_build<K, V>(
    conn: &mut sqlx::PgConnection,
    name: K,
    version: V,
    errors: &str,
) -> Result<(ReleaseId, BuildId)>
where
    K: TryInto<KrateName>,
    K::Error: std::error::Error + Send + Sync + 'static,
    V: TryInto<Version>,
    V::Error: std::error::Error + Send + Sync + 'static,
{
    let name = name.try_into()?;
    let version = version.try_into()?;
    let crate_id = initialize_crate(&mut *conn, &name).await?;
    let release_id = initialize_release(&mut *conn, crate_id, &version).await?;
    let build_id = initialize_build(&mut *conn, release_id).await?;

    sqlx::query_scalar!(
        "UPDATE builds
         SET
             build_status = 'failure',
             errors = $2
         WHERE id = $1",
        build_id.0,
        errors,
    )
    .execute(&mut *conn)
    .await?;

    update_build_status(conn, release_id).await?;

    Ok((release_id, build_id))
}

#[must_use = "FakeRelease does nothing until you call .create()"]
pub struct FakeRelease<'a> {
    pool: Pool,
    storage: Arc<AsyncStorage>,
    package: MetadataPackage,
    builds: Option<Vec<FakeBuild>>,
    /// name, content
    source_files: Vec<(&'a str, &'a [u8])>,
    /// name, content
    rustdoc_files: Vec<(&'a str, &'a [u8])>,
    doc_targets: Vec<String>,
    default_target: Option<&'a str>,
    registry_crate_data: CrateData,
    registry_release_data: ReleaseData,
    has_docs: bool,
    has_examples: bool,
    archive_storage: bool,
    /// This stores the content, while `package.readme` stores the filename
    readme: Option<&'a str>,
    github_stats: Option<FakeGithubStats>,
    doc_coverage: Option<DocCoverage>,
    no_cargo_toml: bool,
}

pub struct FakeBuild {
    s3_build_log: Option<String>,
    other_build_logs: HashMap<String, String>,
    db_build_log: Option<String>,
    rustc_version: String,
    docsrs_version: String,
    build_status: BuildStatus,
}

const DEFAULT_CONTENT: &[u8] =
    b"<html><head></head><body>default content for test/fakes</body></html>";

impl<'a> FakeRelease<'a> {
    pub fn new(pool: Pool, storage: Arc<AsyncStorage>) -> Self {
        FakeRelease {
            pool,
            storage,
            package: MetadataPackage {
                id: "fake-package-id".into(),
                name: "fake-package".into(),
                version: Version::new(1, 0, 0),
                license: Some("MIT".into()),
                repository: Some("https://git.example.com".into()),
                homepage: Some("https://www.example.com".into()),
                description: Some("Fake package".into()),
                documentation: Some("https://docs.example.com".into()),
                dependencies: vec![Dependency {
                    name: "fake-dependency".into(),
                    req: VersionReq::parse("^1.0.0").unwrap(),
                    kind: None,
                    rename: None,
                    optional: false,
                }],
                targets: vec![Target::dummy_lib("fake_package".into(), None)],
                readme: None,
                keywords: vec!["fake".into(), "package".into()],
                features: [
                    ("default".into(), vec!["feature1".into(), "feature3".into()]),
                    ("feature1".into(), Vec::new()),
                    ("feature2".into(), vec!["feature1".into()]),
                    ("feature3".into(), Vec::new()),
                ]
                .iter()
                .cloned()
                .collect::<BTreeMap<String, Vec<String>>>(),
            },
            builds: None,
            source_files: Vec::new(),
            rustdoc_files: Vec::new(),
            doc_targets: Vec::new(),
            default_target: None,
            registry_crate_data: CrateData { owners: Vec::new() },
            registry_release_data: ReleaseData {
                release_time: Utc::now(),
                yanked: false,
                downloads: 0,
            },
            has_docs: true,
            has_examples: false,
            readme: None,
            github_stats: None,
            doc_coverage: None,
            archive_storage: false,
            no_cargo_toml: false,
        }
    }

    pub fn description(mut self, new: impl Into<String>) -> Self {
        self.package.description = Some(new.into());
        self
    }

    pub fn add_dependency(mut self, dependency: Dependency) -> Self {
        self.package.dependencies.push(dependency);
        self
    }

    pub fn release_time(mut self, new: DateTime<Utc>) -> Self {
        self.registry_release_data.release_time = new;
        self
    }

    pub fn name<K>(mut self, new: K) -> Self
    where
        K: TryInto<KrateName>,
        K::Error: fmt::Debug,
    {
        let new = new.try_into().expect("invalid crate name").to_string();

        self.package.name = new.clone();
        self.package.id = format!("{new}-id");
        self.package.targets[0].name = new;
        self
    }

    pub fn version<V>(mut self, new: V) -> Self
    where
        V: TryInto<Version>,
        V::Error: fmt::Debug,
    {
        self.package.version = new.try_into().expect("invalid version");
        self
    }

    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.package.repository = Some(repo.into());
        self
    }

    /// Shortcut to add a single unsuccessful build with default data
    // TODO: How should `has_docs` actually be handled?
    pub fn build_result_failed(self) -> Self {
        assert!(
            self.builds.is_none(),
            "cannot use custom builds with build_result_failed"
        );
        Self {
            has_docs: false,
            builds: Some(vec![FakeBuild::default().successful(false)]),
            ..self
        }
    }

    pub fn builds(self, builds: Vec<FakeBuild>) -> Self {
        assert!(self.builds.is_none());
        assert!(!builds.is_empty());
        Self {
            builds: Some(builds),
            ..self
        }
    }

    pub fn no_builds(self) -> Self {
        assert!(self.builds.is_none());
        Self {
            builds: Some(vec![]),
            ..self
        }
    }

    pub fn yanked(mut self, new: bool) -> Self {
        self.registry_release_data.yanked = new;
        self
    }

    pub fn archive_storage(mut self, new: bool) -> Self {
        self.archive_storage = new;
        self
    }

    /// Since we switched to LOL HTML, all data must have a valid <head> and <body>.
    /// To avoid duplicating them in every test, this just makes up some content.
    pub fn rustdoc_file(mut self, path: &'a str) -> Self {
        self.rustdoc_files.push((path, DEFAULT_CONTENT));
        self
    }

    pub fn rustdoc_file_with(mut self, path: &'a str, data: &'a [u8]) -> Self {
        self.rustdoc_files.push((path, data));
        self
    }

    pub fn source_file(mut self, path: &'a str, data: &'a [u8]) -> Self {
        self.source_files.push((path, data));
        self
    }

    pub fn target_source(mut self, path: &'a str) -> Self {
        if let Some(target) = self.package.targets.first_mut() {
            target.src_path = Some(path.into());
        }
        self
    }

    pub fn no_cargo_toml(mut self) -> Self {
        self.no_cargo_toml = true;
        self
    }

    pub fn default_target(mut self, target: &'a str) -> Self {
        self = self.add_target(target);
        self.default_target = Some(target);
        self
    }

    pub fn add_target(mut self, target: &str) -> Self {
        self.doc_targets.push(target.into());
        self
    }

    pub fn binary(mut self, bin: bool) -> Self {
        self.has_docs = !bin;
        if bin {
            for target in self.package.targets.iter_mut() {
                target.crate_types = vec!["bin".into()];
            }
        }
        self
    }

    pub fn add_platform<S: Into<String>>(mut self, platform: S) -> Self {
        let platform = platform.into();
        let name = self.package.targets[0].name.clone();
        let target = Target::dummy_lib(name, Some(platform.clone()));
        self.package.targets.push(target);
        self.doc_targets.push(platform);
        self
    }

    /// NOTE: this should be markdown. It will be rendered as HTML when served.
    pub fn readme(mut self, content: &'a str) -> Self {
        self.readme = Some(content);
        self.source_file("README.md", content.as_bytes())
    }

    /// NOTE: this should be markdown. It will be rendered as HTML when served.
    pub fn readme_only_database(mut self, content: &'a str) -> Self {
        self.readme = Some(content);
        self
    }

    pub fn add_owner(mut self, owner: CrateOwner) -> Self {
        self.registry_crate_data.owners.push(owner);
        self
    }

    pub fn doc_coverage(self, doc_coverage: DocCoverage) -> Self {
        Self {
            doc_coverage: Some(doc_coverage),
            ..self
        }
    }

    pub fn features(mut self, features: BTreeMap<String, Vec<String>>) -> Self {
        self.package.features = features;
        self
    }

    pub fn github_stats(
        mut self,
        repo: impl Into<String>,
        stars: i32,
        forks: i32,
        issues: i32,
    ) -> Self {
        self.github_stats = Some(FakeGithubStats {
            repo: repo.into(),
            stars,
            forks,
            issues,
        });
        self
    }

    pub fn documentation_url(mut self, documentation_url: Option<String>) -> Self {
        self.package.documentation = documentation_url;
        self
    }

    /// Returns the release_id
    pub async fn create(mut self) -> Result<ReleaseId> {
        use std::fs;
        use std::path::Path;

        let package = self.package;
        let pool = self.pool;
        let mut rustdoc_files = self.rustdoc_files;
        let storage = self.storage;
        let archive_storage = self.archive_storage;

        // Upload all source files as rustdoc files
        // In real life, these would be highlighted HTML, but for testing we just use the files themselves.
        for (source_path, data) in &self.source_files {
            if let Some(src) = source_path.strip_prefix("src/") {
                let mut updated = ["src", &package.name, src].join("/");
                updated += ".html";
                let source_html = format!(
                    "<html><head></head><body>{}</body></html>",
                    std::str::from_utf8(data).expect("invalid utf8")
                );
                rustdoc_files.push((
                    Box::leak(Box::new(updated)),
                    Box::leak(source_html.into_bytes().into_boxed_slice()),
                ));
            }
        }

        #[derive(Debug)]
        enum FileKind {
            Rustdoc,
            Sources,
        }

        let create_temp_dir = || {
            tempfile::Builder::new()
                .prefix("docs.rs-fake")
                .tempdir()
                .unwrap()
        };

        let store_files_into = |files: &[(&str, &[u8])], base_path: &Path| {
            for (path, data) in files {
                if path.starts_with('/') {
                    anyhow::bail!("absolute paths not supported");
                }
                // allow `src/main.rs`
                if let Some(parent) = Path::new(path).parent() {
                    let path = base_path.join(parent);
                    fs::create_dir_all(&path)
                        .with_context(|| format!("failed to create {}", path.display()))?;
                }
                let file = base_path.join(path);
                debug!("writing file {}", file.display());
                fs::write(file, data)?;
            }
            Ok(())
        };

        async fn upload_files(
            kind: FileKind,
            source_directory: &Path,
            archive_storage: bool,
            package: &MetadataPackage,
            storage: &AsyncStorage,
        ) -> Result<(Vec<FileEntry>, CompressionAlgorithm)> {
            debug!(
                "adding directory {:?} from {}",
                kind,
                source_directory.display()
            );
            if archive_storage {
                // NOTE: should we migrate MetadataPackage?
                let krate_name: KrateName = package.name.parse()?;

                let archive = match kind {
                    FileKind::Rustdoc => rustdoc_archive_path(&krate_name, &package.version),
                    FileKind::Sources => source_archive_path(&krate_name, &package.version),
                };
                debug!("store in archive: {:?}", archive);
                Ok(storage
                    .store_all_in_archive(&archive, source_directory)
                    .await?)
            } else {
                let prefix = match kind {
                    FileKind::Rustdoc => "rustdoc",
                    FileKind::Sources => "sources",
                };
                storage
                    .store_all(
                        format!("{}/{}/{}/", prefix, package.name, package.version),
                        source_directory,
                    )
                    .await
            }
        }

        debug!("before upload source");
        let source_tmp = create_temp_dir();
        store_files_into(&self.source_files, source_tmp.path())?;

        if !self.no_cargo_toml
            && !self
                .source_files
                .iter()
                .any(|&(path, _)| path == "Cargo.toml")
        {
            let MetadataPackage { name, version, .. } = &package;
            let content = format!(
                r#"
                [package]
                name = "{name}"
                version = "{version}"
            "#
            );
            store_files_into(&[("Cargo.toml", content.as_bytes())], source_tmp.path())?;
        }

        let (source_meta, algs) = upload_files(
            FileKind::Sources,
            source_tmp.path(),
            archive_storage,
            &package,
            &storage,
        )
        .await?;
        debug!(?source_meta, "added source files");

        // If the test didn't add custom builds, inject a default one
        let builds = self.builds.unwrap_or_else(|| vec![FakeBuild::default()]);

        if builds.last().map(|b| b.build_status) == Some(BuildStatus::Success) {
            let index = [&package.name, "index.html"].join("/");
            if package.is_library() && !rustdoc_files.iter().any(|(path, _)| path == &index) {
                rustdoc_files.push((&index, DEFAULT_CONTENT));
            }

            let rustdoc_tmp = create_temp_dir();
            let rustdoc_path = rustdoc_tmp.path();

            // store default target files
            store_files_into(&rustdoc_files, rustdoc_path)?;
            debug!("added rustdoc files");

            for target in &package.targets[1..] {
                let platform = target.src_path.as_ref().unwrap();
                let platform_dir = rustdoc_path.join(platform);
                fs::create_dir(&platform_dir)?;

                store_files_into(&rustdoc_files, &platform_dir)?;
                debug!("added platform files for {}", platform);
            }

            let (files, _) = upload_files(
                FileKind::Rustdoc,
                rustdoc_path,
                archive_storage,
                &package,
                &storage,
            )
            .await?;
            debug!(?files, "uploaded rustdoc files");
        }

        let mut async_conn = pool.get_async().await?;

        let repository = match self.github_stats {
            Some(stats) => Some(stats.create(&mut async_conn).await?),
            None => None,
        };

        let crate_tmp = create_temp_dir();
        let crate_dir = crate_tmp.path();
        if let Some(markdown) = self.readme {
            fs::write(crate_dir.join("README.md"), markdown)?;
        }
        store_files_into(&self.source_files, crate_dir)?;

        let default_target = self.default_target.unwrap_or("x86_64-unknown-linux-gnu");
        if !self.doc_targets.iter().any(|t| t == default_target) {
            self.doc_targets.insert(0, default_target.to_owned());
        }

        let krate_name: KrateName = package.name.parse()?;

        for target in &self.doc_targets {
            let dummy_rustdoc_json_content = serde_json::to_vec(&serde_json::json!({
                "format_version": 42
            }))?;

            for alg in RUSTDOC_JSON_COMPRESSION_ALGORITHMS {
                let compressed_json: Vec<u8> = compress(&*dummy_rustdoc_json_content, *alg)?;

                for format_version in [
                    RustdocJsonFormatVersion::Version(42),
                    RustdocJsonFormatVersion::Latest,
                ] {
                    storage
                        .store_one_uncompressed(
                            &rustdoc_json_path(
                                &krate_name,
                                &package.version,
                                target,
                                format_version,
                                Some(*alg),
                            ),
                            compressed_json.clone(),
                        )
                        .await?;
                }
            }
        }

        // Many tests rely on the default-target being linux, so it should not
        // be set to docsrs_metadata::HOST_TARGET, because then tests fail on all
        // non-linux platforms.
        let mut async_conn = pool.get_async().await?;
        let crate_id = initialize_crate(&mut async_conn, &krate_name).await?;
        let release_id = initialize_release(&mut async_conn, crate_id, &package.version).await?;

        docs_rs_database::releases::finish_release(
            &mut async_conn,
            crate_id,
            release_id,
            &package,
            crate_dir,
            default_target,
            file_list_to_json(source_meta),
            self.doc_targets,
            &self.registry_release_data,
            self.has_docs,
            self.has_examples,
            iter::once(algs),
            repository,
            archive_storage,
            24,
        )
        .await?;
        docs_rs_database::releases::update_crate_data_in_database(
            &mut async_conn,
            &krate_name,
            &self.registry_crate_data,
        )
        .await?;
        for build in builds {
            build
                .create(&mut async_conn, &storage, release_id, default_target)
                .await?;
        }
        if let Some(coverage) = self.doc_coverage {
            docs_rs_database::releases::add_doc_coverage(&mut async_conn, release_id, coverage)
                .await?;
        }

        Ok(release_id)
    }
}

pub struct FakeGithubStats {
    pub repo: String,
    pub stars: i32,
    pub forks: i32,
    pub issues: i32,
}

impl FakeGithubStats {
    pub async fn create(&self, conn: &mut sqlx::PgConnection) -> Result<i32> {
        let existing_count: i64 = sqlx::query_scalar!("SELECT COUNT(*) FROM repositories")
            .fetch_one(&mut *conn)
            .await?
            .unwrap();
        let host_id = b64.encode(format!("FAKE ID {existing_count}"));

        let id = sqlx::query_scalar!(
            "INSERT INTO repositories (host, host_id, name, description, last_commit, stars, forks, issues, updated_at)
             VALUES ('github.com', $1, $2, 'Fake description!', NOW(), $3, $4, $5, NOW())
             RETURNING id",
            host_id, self.repo, self.stars, self.forks, self.issues,
        ).fetch_one(&mut *conn).await?;

        Ok(id)
    }
}

impl FakeBuild {
    pub fn rustc_version(self, rustc_version: impl Into<String>) -> Self {
        Self {
            rustc_version: rustc_version.into(),
            ..self
        }
    }

    pub fn docsrs_version(self, docsrs_version: impl Into<String>) -> Self {
        Self {
            docsrs_version: docsrs_version.into(),
            ..self
        }
    }

    pub fn s3_build_log(self, build_log: impl Into<String>) -> Self {
        Self {
            s3_build_log: Some(build_log.into()),
            ..self
        }
    }

    pub fn build_log_for_other_target(
        mut self,
        target: impl Into<String>,
        build_log: impl Into<String>,
    ) -> Self {
        self.other_build_logs
            .insert(target.into(), build_log.into());
        self
    }

    pub fn db_build_log(self, build_log: impl Into<String>) -> Self {
        Self {
            db_build_log: Some(build_log.into()),
            ..self
        }
    }

    pub fn no_s3_build_log(self) -> Self {
        Self {
            s3_build_log: None,
            ..self
        }
    }

    pub fn successful(self, successful: bool) -> Self {
        self.build_status(if successful {
            BuildStatus::Success
        } else {
            BuildStatus::Failure
        })
    }

    pub fn build_status(self, build_status: BuildStatus) -> Self {
        Self {
            build_status,
            ..self
        }
    }

    async fn create(
        &self,
        conn: &mut sqlx::PgConnection,
        storage: &AsyncStorage,
        release_id: ReleaseId,
        default_target: &str,
    ) -> Result<()> {
        let build_id = docs_rs_database::releases::initialize_build(&mut *conn, release_id).await?;

        docs_rs_database::releases::finish_build(
            &mut *conn,
            build_id,
            &self.rustc_version,
            &self.docsrs_version,
            self.build_status,
            Some(42),
            None,
        )
        .await?;

        if let Some(db_build_log) = self.db_build_log.as_deref() {
            sqlx::query!(
                "UPDATE builds SET output = $2 WHERE id = $1",
                build_id.0,
                db_build_log
            )
            .execute(&mut *conn)
            .await?;
        }

        let prefix = format!("build-logs/{build_id}/");

        if let Some(s3_build_log) = self.s3_build_log.as_deref() {
            let path = format!("{prefix}{default_target}.txt");
            storage.store_one(path, s3_build_log).await?;
        }

        for (target, log) in &self.other_build_logs {
            if target == default_target {
                bail!("build log for default target has to be set via `s3_build_log`");
            }
            let path = format!("{prefix}{target}.txt");
            storage.store_one(path, log.as_str()).await?;
        }

        Ok(())
    }
}

impl Default for FakeBuild {
    /// create a default fake _finished_ build
    fn default() -> Self {
        Self {
            s3_build_log: Some("It works!".into()),
            db_build_log: None,
            other_build_logs: HashMap::new(),
            rustc_version: "rustc 2.0.0-nightly (000000000 1970-01-01)".into(),
            docsrs_version: "docs.rs 1.0.0 (000000000 1970-01-01)".into(),
            build_status: BuildStatus::Success,
        }
    }
}
