use crate::{
    common::{DOCS_RS, download, download_to_temp_file},
    crates_io::download_and_extract_source,
    rustdoc::{download_static_files, find_static_paths, find_successful_build_targets},
    rustdoc_status::fetch_rustdoc_status,
};
use anyhow::{Result, bail};
use docs_rs_cargo_metadata::CargoMetadata;
use docs_rs_database::releases::{
    finish_build, finish_release, initialize_build, initialize_crate, initialize_release,
    update_build_with_error,
};
use docs_rs_registry_api::RegistryApi;
use docs_rs_repository_stats::RepositoryStatsUpdater;
use docs_rs_rustdoc_json::{
    RUSTDOC_JSON_COMPRESSION_ALGORITHMS, RustdocJsonFormatVersion,
    read_format_version_from_rustdoc_json,
};
use docs_rs_storage::{AsyncStorage, file_list_to_json, rustdoc_archive_path, source_archive_path};
use docs_rs_storage::{compress, decompress, rustdoc_json_path};
use docs_rs_types::{BuildId, BuildStatus, CrateId, KrateName, ReleaseId, ReqVersion, Version};
use docs_rs_utils::{BUILD_VERSION, spawn_blocking};
use docsrs_metadata::Metadata;
use std::collections::HashSet;
use tracing::{info, instrument};

const DEFAULT_TARGET: &str = "x86_64-unknown-linux-gnu";

/// import an existing crate release build from docs.rs into the
/// local database & storage.
///
/// CAVEATS:
/// * is currently only tested for newer releases, since there are some hacks in place.
/// * to find the needed rustdoc-static files, we have to scan all the HTML files for certain paths.
///   For bigger releases this might take some time.
/// * we assume when the normal target build is successfull, we also have a valid rustdoc json file,
///   and we'll ignore any rustdoc JSON files related to failed targets.
/// * build logs are fake, but are created.
///
/// SECURITY:
/// we execute `cargo metadata` on the downloaded source code, so
/// this function MUST NOT be used with untrusted crate names/versions.
pub(crate) async fn import_test_release(
    conn: &mut sqlx::PgConnection,
    storage: &AsyncStorage,
    registry_api: &RegistryApi,
    repository_stats: &RepositoryStatsUpdater,
    name: &KrateName,
    version: &ReqVersion,
) -> Result<()> {
    let status = fetch_rustdoc_status(name, version).await?;
    if !status.doc_status {
        bail!("No rustdoc available for {name} {version}");
    }
    let version = status.version;

    let crate_id = initialize_crate(&mut *conn, name).await?;
    let release_id = initialize_release(&mut *conn, crate_id, &version).await?;
    let build_id = initialize_build(&mut *conn, release_id).await?;

    let result = import_test_release_inner(
        &mut *conn,
        storage,
        registry_api,
        repository_stats,
        name,
        &version,
        crate_id,
        release_id,
        build_id,
    )
    .await;

    if let Err(err) = &result {
        update_build_with_error(&mut *conn, build_id, Some(&format!("{err:?}"))).await?;
    }

    result
}

#[allow(clippy::too_many_arguments)]
#[instrument(skip_all, fields(name=%name, version=%version))]
async fn import_test_release_inner(
    conn: &mut sqlx::PgConnection,
    storage: &AsyncStorage,
    registry_api: &RegistryApi,
    repository_stats: &RepositoryStatsUpdater,
    name: &KrateName,
    version: &Version,
    crate_id: CrateId,
    release_id: ReleaseId,
    build_id: BuildId,
) -> Result<()> {
    info!("download & inspect source from crates.io...");
    let source_dir = download_and_extract_source(name, version).await?;

    let cargo_metadata = spawn_blocking({
        let source_dir = source_dir.source_path.clone();
        move || CargoMetadata::load_from_host_path(&source_dir)
    })
    .await?;
    let docsrs_metadata = spawn_blocking({
        let source_dir = source_dir.source_path.clone();
        move || Ok(Metadata::from_crate_root(&source_dir)?)
    })
    .await?;

    let mut algs = HashSet::new();
    let (source_files_list, source_size) = {
        info!("writing source files to storage...");
        let (files_list, new_alg) = storage
            .store_all_in_archive(&source_archive_path(name, version), &source_dir)
            .await?;

        algs.insert(new_alg);
        let source_size: u64 = files_list.iter().map(|info| info.size).sum();
        (files_list, source_size)
    };

    let registry_data = registry_api.get_release_data(name, version).await?;

    let rustdoc_dir = {
        info!("download & extract rustdoc archive...");
        let rustdoc_archive =
            download_to_temp_file(format!("{DOCS_RS}/crate/{name}/{version}/download"))
                .await?
                .into_std()
                .await;

        spawn_blocking(|| {
            let mut zip = zip::ZipArchive::new(rustdoc_archive)?;

            let temp_dir = tempfile::tempdir()?;
            zip.extract(&temp_dir)?;
            Ok(temp_dir)
        })
        .await?
    };

    info!("find successfull build targets...");
    let (default_target, all_targets) = {
        let build_targets = docsrs_metadata.targets_for_host(true, DEFAULT_TARGET);
        (
            build_targets.default_target,
            find_successful_build_targets(
                &rustdoc_dir,
                build_targets.default_target,
                build_targets.other_targets,
            )
            .await?,
        )
    };

    info!("uploading fake build logs");
    for build_target in &all_targets {
        storage
            .store_one(
                format!("build-logs/{build_id}/{build_target}.txt"),
                format!("fake build output\nbuild target: {}", build_target),
            )
            .await?;
    }

    info!("finding used rustdoc static files in HTML...");
    {
        let static_files = find_static_paths(&rustdoc_dir).await?;
        download_static_files(storage, static_files.iter().map(AsRef::as_ref)).await?;
    }

    info!("writing rustdoc files to storage...");
    let (rustdoc_file_list, new_alg) = storage
        .store_all_in_archive(&rustdoc_archive_path(name, version), &rustdoc_dir)
        .await?;
    let documentation_size: u64 = rustdoc_file_list.iter().map(|info| info.size).sum();
    algs.insert(new_alg);

    info!("loading repository stats...");
    let repository_id = repository_stats
        .load_repository(cargo_metadata.root())
        .await?;

    for target in &all_targets {
        info!("copying rustdoc json for target {target}...");

        let json_compression = RUSTDOC_JSON_COMPRESSION_ALGORITHMS[0];
        let rustdoc_json = decompress(
            &*download(format!(
                "{DOCS_RS}/crate/{name}/{version}/{target}/json.{}",
                json_compression.file_extension()
            ))
            .await?,
            json_compression,
            usize::MAX,
        )?;
        if rustdoc_json.is_empty() || rustdoc_json[0] != b'{' {
            bail!("invalid rustdoc json for {name} {version} {target}");
        }

        let format_version = spawn_blocking({
            let rustdoc_json = rustdoc_json.clone();
            move || read_format_version_from_rustdoc_json(&*rustdoc_json)
        })
        .await?;

        for alg in RUSTDOC_JSON_COMPRESSION_ALGORITHMS {
            let compressed_json = compress(&*rustdoc_json, *alg)?;

            for format_version in [format_version, RustdocJsonFormatVersion::Latest] {
                let path = rustdoc_json_path(name, version, target, format_version, Some(*alg));
                storage
                    .store_one_uncompressed(&path, compressed_json.clone())
                    .await?;
            }
        }
    }

    info!("finish release & build");
    finish_release(
        &mut *conn,
        crate_id,
        release_id,
        cargo_metadata.root(),
        &source_dir,
        default_target,
        file_list_to_json(source_files_list),
        all_targets,
        &registry_data,
        true,
        false, // FIXME: real has_examples?
        algs,
        repository_id,
        true,
        source_size,
    )
    .await?;

    finish_build(
        &mut *conn,
        build_id,
        "rustc 1.95.0-nightly (873d4682c 2026-01-25)",
        BUILD_VERSION,
        BuildStatus::Success,
        Some(documentation_size),
        None,
    )
    .await?;

    Ok(())
}
