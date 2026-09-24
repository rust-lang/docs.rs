use super::*;
use docs_rs_config::AppConfig as _;
use docs_rs_types::ByteSize;
use pretty_assertions::assert_eq;

fn environment() -> Result<TestEnvironment> {
    let mut config = Config::test_config()?;
    config.include_default_targets = false;
    TestEnvironment::builder().config(config).build()
}

#[derive(Debug, sqlx::FromRow)]
pub(super) struct PublishedBuild {
    pub id: BuildId,
    pub status: BuildStatus,
    pub errors: Option<String>,
    pub rustdoc_status: Option<bool>,
    pub doc_targets: Option<sqlx::types::Json<Vec<String>>>,
}

pub(super) fn fetch_build_result(
    env: &TestEnvironment,
    name: &KrateName,
) -> Result<PublishedBuild> {
    env.runtime().block_on(async {
        let mut conn = env.pool()?.get_async().await?;
        Ok(sqlx::query_as::<_, PublishedBuild>(
            r#"
            SELECT
                b.id,
                b.build_status AS status,
                b.errors,
                r.rustdoc_status,
                r.doc_targets
            FROM builds b
            JOIN releases r ON r.id = b.rid
            JOIN crates c ON c.id = r.crate_id
            WHERE
                c.name = $1
            ORDER BY b.id DESC
            LIMIT 1
        "#,
        )
        .bind(name)
        .fetch_one(&mut *conn)
        .await?)
    })
}

pub(super) fn fetch_build_logs(
    env: &TestEnvironment,
    build_id: BuildId,
) -> Result<Vec<(String, bool)>> {
    env.runtime().block_on(async {
        let mut conn = env.pool()?.get_async().await?;
        Ok(sqlx::query_as(
            r#"
            SELECT
                log_filename,
                success
            FROM builds_logs
            WHERE build_id = $1
            ORDER BY log_filename
        "#,
        )
        .bind(build_id)
        .fetch_all(&mut *conn)
        .await?)
    })
}

// Exercise publication and its production error-to-reattempt mapping after a test
// has inspected or modified the completed build's artifacts.
fn publish_release(
    env: &TestEnvironment,
    builder: &RustwideBuilder,
    name: &KrateName,
    release: BuiltRelease,
) -> Result<BuildPackageSummary> {
    let (crate_id, release_id, build_id) = env.runtime().block_on(async {
        let mut conn = env.pool()?.get_async().await?;
        let crate_id = initialize_crate(&mut conn, name).await?;
        let release_id = initialize_release(&mut conn, crate_id, &V0_1).await?;
        let build_id = initialize_build(&mut conn, release_id).await?;
        Ok::<_, Error>((crate_id, release_id, build_id))
    })?;
    let result = builder.publish_release(name, &V0_1, crate_id, release_id, build_id, release);
    builder.finish_package_build(build_id, result)
}

#[test]
#[ignore]
fn invalid_json_format_is_nonfatal() -> Result<()> {
    let env = environment()?;
    let blocking_storage = env.blocking_storage()?;
    let name = KrateName::from_static("invalid-json-format");
    mock_package(&env, &name, &V0_1, Some("lib.rs"), "pub fn example() {}")?;
    let mut builder = env.build_builder()?;
    let release = builder.build_release(&name, &V0_1)?.unwrap();
    fs::write(
        release
            .result
            .default_target()
            .rustdoc_json()
            .as_inner()
            .expect("JSON build must succeed before corrupting its output")
            .path(),
        b"{}",
    )?;

    let summary = publish_release(&env, &builder, &name, release)?;
    assert!(summary.successful);
    assert!(!summary.should_reattempt);
    let row = fetch_build_result(&env, &name)?;
    assert_eq!(row.status, BuildStatus::Success);
    assert_eq!(row.rustdoc_status, Some(true));
    assert!(blocking_storage.exists_in_archive(
        &rustdoc_archive_path(&name, &V0_1),
        None,
        "invalid_json_format/index.html"
    )?);
    let entries = fetch_build_logs(&env, row.id)?;
    assert_eq!(
        entries
            .iter()
            .filter(|(path, _)| path.ends_with("_json.txt"))
            .count(),
        1
    );
    for (path, _) in entries {
        assert!(blocking_storage.exists(&format!("build-logs/{}/{path}", row.id))?);
    }
    assert!(
        blocking_storage
            .list_prefix(&format!("rustdoc-json/{name}/"))
            .next()
            .is_none()
    );
    Ok(())
}

#[test_case::test_case(
    |p| p.starts_with("rustdoc-json/"), false;
    "json_upload_failure_is_nonfatal"
)]
#[test_case::test_case(
    |p| p.starts_with("build-logs/") && p.ends_with("_json.txt"), true;
    "json_log_upload_failure_requests_reattempt"
)]
#[test_case::test_case(
    |p| p.starts_with("rustdoc/"), true;
    "html_upload_failure_requests_reattempt"
)]
#[test_case::test_case(
    |p| p.starts_with("build-logs/") && !p.ends_with("_json.txt"), true;
    "html_log_upload_failure_requests_reattempt"
)]
#[ignore]
fn upload_failure(reject: fn(&str) -> bool, fatal: bool) -> Result<()> {
    let env = environment()?;
    let storage = env.storage()?;
    let blocking_storage = env.blocking_storage()?;
    let name = KrateName::from_static("publication-failure");
    mock_package(&env, &name, &V0_1, Some("lib.rs"), "pub fn example() {}")?;
    let mut builder = env.build_builder()?;
    storage.reject_uploads_for_testing(Some(reject));
    let summary = builder.build_package(&name, &V0_1)?;
    assert_eq!(summary.successful, !fatal);
    assert_eq!(summary.should_reattempt, fatal);
    let row = fetch_build_result(&env, &name)?;
    assert_eq!(
        row.status,
        if fatal {
            BuildStatus::Failure
        } else {
            BuildStatus::Success
        }
    );
    if fatal {
        assert!(row.errors.unwrap().contains("injected upload failure"));
    } else {
        assert_eq!(row.rustdoc_status, Some(true));
        assert!(blocking_storage.exists_in_archive(
            &rustdoc_archive_path(&name, &V0_1),
            None,
            "publication_failure/index.html"
        )?);
        let entries = fetch_build_logs(&env, row.id)?;
        assert_eq!(
            entries
                .iter()
                .filter(|(p, _)| p.ends_with("_json.txt"))
                .count(),
            1
        );
        for (path, _) in entries {
            assert!(blocking_storage.exists(&format!("build-logs/{}/{path}", row.id))?);
        }
        assert!(
            blocking_storage
                .list_prefix(&format!("rustdoc-json/{name}/"))
                .next()
                .is_none()
        );
    }
    storage.reject_uploads_for_testing(None);
    Ok(())
}

#[test]
#[ignore]
fn failed_additional_target_is_excluded_from_publication() -> Result<()> {
    let env = environment()?;
    let blocking_storage = env.blocking_storage()?;
    let name = KrateName::from_static("partial-targets");
    mock_package_with_manifest(
        &env,
        &name,
        &V0_1,
        Some("lib.rs"),
        "pub fn example() {}",
        "[package.metadata.docs.rs]\ntargets = [\"x86_64-unknown-linux-gnu\", \"docsrs-invalid-target\"]\n",
        true,
    )?;
    let summary = env.build_builder()?.build_package(&name, &V0_1)?;
    assert!(summary.successful);
    assert!(!summary.should_reattempt);
    let row = fetch_build_result(&env, &name)?;
    assert_eq!(
        row.doc_targets.unwrap().0,
        vec!["x86_64-unknown-linux-gnu".to_string()]
    );
    let entries = fetch_build_logs(&env, row.id)?;
    assert!(entries.contains(&("docsrs-invalid-target.txt".into(), false)));
    assert!(blocking_storage.exists(&format!("build-logs/{}/docsrs-invalid-target.txt", row.id))?);
    assert!(blocking_storage.exists_in_archive(
        &rustdoc_archive_path(&name, &V0_1),
        None,
        "partial_targets/index.html"
    )?);
    Ok(())
}

#[test]
#[ignore]
fn command_failure_is_recorded_without_queue_reattempt() -> Result<()> {
    let env = environment()?;
    let runtime = env.runtime();
    let storage = env.storage()?;
    let name = KrateName::from_static("command-failure");
    mock_package(
        &env,
        &name,
        &V0_1,
        Some("lib.rs"),
        "compile_error!(\"intentional compile failure\");",
    )?;
    let summary = env.build_builder()?.build_package(&name, &V0_1)?;
    assert!(!summary.successful);
    assert!(!summary.should_reattempt);
    let row = fetch_build_result(&env, &name)?;
    assert_eq!(row.status, BuildStatus::Failure);
    assert!(row.errors.is_some());
    let entries = fetch_build_logs(&env, row.id)?;
    assert!(!entries.is_empty());
    for (filename, success) in entries {
        assert!(!success);
        let blob = runtime
            .block_on(storage.get(&format!("build-logs/{}/{filename}", row.id), ByteSize::MAX))?;
        assert!(String::from_utf8(blob.content)?.contains("intentional compile failure"));
    }
    Ok(())
}

#[test]
#[ignore]
fn registry_metadata_errors_are_nonfatal() -> Result<()> {
    let env = environment()?;
    let name = KrateName::from_static("missing-registry-metadata");
    // No sparse release record and no crate/owners API mocks: all return errors.
    mock_package_with_manifest(
        &env,
        &name,
        &V0_1,
        Some("lib.rs"),
        "pub fn example() {}",
        "",
        false,
    )?;
    let summary = env.build_builder()?.build_package(&name, &V0_1)?;
    assert!(summary.successful);
    assert!(!summary.should_reattempt);
    assert_eq!(fetch_build_result(&env, &name)?.rustdoc_status, Some(true));
    assert!(
        env.blocking_storage()?
            .exists(&rustdoc_archive_path(&name, &V0_1))?
    );
    Ok(())
}

#[test]
#[ignore]
fn blacklisted_crate_is_skipped_without_reattempt() -> Result<()> {
    let env = environment()?;
    let blocking_storage = env.blocking_storage()?;
    let name = KrateName::from_static("blacklisted-package");
    env.runtime().block_on(async {
        let mut conn = env.pool()?.get_async().await?;
        docs_rs_build_limits::blacklist::add_crate(&mut conn, &name).await?;
        Ok::<_, Error>(())
    })?;
    let summary = env.build_builder()?.build_package(&name, &V0_1)?;
    assert!(!summary.successful);
    assert!(!summary.should_reattempt);
    assert!(!blocking_storage.exists(&source_archive_path(&name, &V0_1))?);
    assert!(!blocking_storage.exists(&rustdoc_archive_path(&name, &V0_1))?);
    assert!(fetch_build_logs(&env, fetch_build_result(&env, &name)?.id)?.is_empty());
    Ok(())
}

#[test]
#[ignore]
fn essential_files_are_published_only_when_needed() -> Result<()> {
    let env = environment()?;
    let runtime = env.runtime();
    let pool = env.pool()?;
    let storage = env.storage()?;
    let mut builder = env.build_builder()?;
    let published = || -> Result<Option<String>> {
        runtime.block_on(async {
            let mut conn = pool.get_async().await?;
            get_config(&mut conn, ConfigName::RustcVersion).await
        })
    };
    assert_eq!(published()?, None);
    storage.reject_uploads_for_testing(Some(|p| p.starts_with(RUSTDOC_STATIC_STORAGE_PREFIX)));
    assert!(builder.publish_essential_files_if_needed(false).is_err());
    assert_eq!(published()?, None);
    storage.reject_uploads_for_testing(None);
    builder.publish_essential_files_if_needed(false)?;
    let version = builder.environment.rustc_version()?;
    assert_eq!(published()?, Some(version.clone()));
    assert!(
        env.blocking_storage()?
            .list_prefix(RUSTDOC_STATIC_STORAGE_PREFIX)
            .next()
            .is_some()
    );
    storage.reject_uploads_for_testing(Some(|p| p.starts_with(RUSTDOC_STATIC_STORAGE_PREFIX)));
    // A matching version must skip uploading; an update must still publish.
    builder.publish_essential_files_if_needed(false)?;
    assert!(builder.publish_essential_files_if_needed(true).is_err());
    runtime.block_on(async {
        let mut conn = pool.get_async().await?;
        set_config(&mut conn, ConfigName::RustcVersion, &"outdated").await
    })?;
    assert!(builder.publish_essential_files_if_needed(false).is_err());
    assert_eq!(published()?, Some("outdated".into()));
    storage.reject_uploads_for_testing(None);
    builder.publish_essential_files_if_needed(false)?;
    assert_eq!(published()?, Some(version));
    Ok(())
}

#[test]
#[ignore]
fn regeneration_failure_does_not_request_queue_reattempt() -> Result<()> {
    let env = environment()?;
    let runtime = env.runtime();
    let storage = env.storage()?;
    let name = KrateName::from_static("regeneration-failure");
    mock_package(&env, &name, &V0_1, Some("lib.rs"), "pub fn example() {}")?;
    let mut builder = env.build_builder()?;
    let builds = env.config().rustwide_workspace.join("builds");
    let limits = builder.get_limits(&name)?;
    let krate = Crate::sparse_registry(
        builder.registry_config.sparse_index_host.clone(),
        name.as_str(),
        &V0_1.to_string(),
    )?;
    fs::create_dir_all(&builder.config.temp_dir)?;
    let source_dir = tempfile::tempdir_in(&builder.config.temp_dir)?;
    let fetched = builder
        .environment
        .release(&krate)
        .directory_label(format!("{name}-{V0_1}"))
        .limits(limits)
        .fetch()?;
    fetched.copy_source_to(source_dir.path())?;
    let source_stats = runtime
        .block_on(storage.store_all_in_archive(&source_archive_path(&name, &V0_1), &source_dir))?;
    let full_build_result = fetched.run(|build| {
        // The callback runs after initial metadata/fetch have succeeded.
        // The exclusive workspace contains exactly one active release build.
        let sources: Vec<_> = fs::read_dir(&builds)?
            .map(|entry| entry.map(|entry| entry.path().join("source")))
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter()
            .filter(|path| path.is_dir())
            .collect();
        assert_eq!(sources.len(), 1);
        assert!(sources[0].join("Cargo.lock").exists());
        fs::write(sources[0].join("Cargo.toml"), "[")?;
        Ok(build.build_docs())
    })?;
    let release = BuiltRelease {
        statistics: full_build_result.statistics().clone(),
        result: full_build_result.into_inner(),
        source_dir,
        source_stats,
    };
    let target = release.result.default_target();
    assert!(target.documentation().is_err());
    let failure = target
        .regenerate_lockfile()
        .expect("regeneration must be attempted")
        .as_ref()
        .expect_err("regeneration must actually fail");
    assert!(failure.log().unwrap().contains("Cargo.toml"));
    let summary = publish_release(&env, &builder, &name, release)?;
    assert!(!summary.successful);
    assert!(!summary.should_reattempt);
    let row = fetch_build_result(&env, &name)?;
    assert_eq!(row.status, BuildStatus::Failure);
    assert!(row.errors.is_some());
    let entries = fetch_build_logs(&env, row.id)?;
    assert!(!entries.is_empty());
    for (filename, success) in entries {
        assert!(!success);
        let log = runtime
            .block_on(storage.get(&format!("build-logs/{}/{filename}", row.id), ByteSize::MAX))?;
        assert!(String::from_utf8(log.content)?.contains("Cargo.toml"));
    }
    Ok(())
}
