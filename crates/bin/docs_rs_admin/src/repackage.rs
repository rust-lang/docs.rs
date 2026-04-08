use anyhow::Result;
use docs_rs_storage::{AsyncStorage, FileEntry, rustdoc_archive_path, source_archive_path};
use docs_rs_types::{CompressionAlgorithm, KrateName, ReleaseId, Version};
use docs_rs_utils::{retry_async, spawn_blocking};
use futures_util::TryStreamExt as _;
use sqlx::Acquire as _;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::{fs, io};
use tracing::{debug, info, instrument};

/// repackage old rustdoc / source content.
///
/// New releases are storaged as ZIP files for quite some time already,
/// from the current 1.9 million releases, only 363k are old non-archive
/// releases, where we store all the single files on the storage.
///
/// Since I don't want to rebuild all of these,
/// and I don't even know if stuff that old can be rebuilt with current toolchains,
/// I'll just repackage the old file.
///
/// So
/// 1. download all files for rustdoc / source from storage
/// 2. create a ZIP archive containing all these files
/// 3. upload the zip
/// 4. update database entries accordingly
/// 5. delete old files
///
/// When that's done, I can remove all the logic in the codebase related to
/// non-archive storage.
#[instrument(skip_all, fields(rid=%rid, name=%name, version=%version))]
pub async fn repackage(
    conn: &mut sqlx::PgConnection,
    storage: &AsyncStorage,
    rid: ReleaseId,
    name: &KrateName,
    version: &Version,
) -> Result<()> {
    info!("repackaging");

    let mut transaction = conn.begin().await?;

    let rustdoc_prefix = format!("rustdoc/{name}/{version}/");
    let rustdoc_archive_path = rustdoc_archive_path(name, version);

    let sources_prefix = format!("sources/{name}/{version}/");
    let source_archive_path = source_archive_path(name, version);

    let mut algs: HashSet<CompressionAlgorithm> = HashSet::new();

    if let Some((_rustdoc_file_list, alg)) =
        repackage_path(storage, &rustdoc_prefix, &rustdoc_archive_path).await?
    {
        algs.insert(alg);
    }

    if let Some((_source_file_list, alg)) =
        repackage_path(storage, &sources_prefix, &source_archive_path).await?
    {
        algs.insert(alg);
    };

    let affected = sqlx::query!(
        r#"
        UPDATE releases
        SET archive_storage = TRUE
        WHERE id = $1;
        "#,
        rid as _,
    )
    .execute(&mut *transaction)
    .await?
    .rows_affected();

    debug_assert!(
        affected > 0,
        "release not found in database. Can't update archive_storage"
    );

    sqlx::query!("DELETE FROM compression_rels WHERE release = $1;", rid as _)
        .execute(&mut *transaction)
        .await?;

    for alg in algs {
        sqlx::query!(
            "INSERT INTO compression_rels (release, algorithm)
             VALUES ($1, $2)
             ON CONFLICT DO NOTHING;",
            rid as _,
            &(alg as i32)
        )
        .execute(&mut *transaction)
        .await?;
    }

    transaction.commit().await?;

    // only delete the old files when we were able to update database with `archive_storage=true`,
    // and were able to validate the zip file.
    info!("removing legacy files from storage...");
    storage.delete_prefix(&rustdoc_prefix).await?;
    storage.delete_prefix(&sources_prefix).await?;

    Ok(())
}

/// repackage contents of a S3 path prefix into a single archive file.
///
/// Not performance optimized, for now it just tries to be simple.
#[instrument(skip(storage))]
async fn repackage_path(
    storage: &AsyncStorage,
    prefix: &str,
    target_archive: &str,
) -> Result<Option<(Vec<FileEntry>, CompressionAlgorithm)>> {
    const DOWNLOAD_CONCURRENCY: usize = 8;

    info!("repackage path");
    let tempdir = spawn_blocking(|| tempfile::tempdir().map_err(Into::into)).await?;
    let tempdir_path = tempdir.path().to_path_buf();

    let files = Arc::new(AtomicUsize::new(0));
    storage
        .list_prefix(prefix)
        .await
        .try_for_each_concurrent(DOWNLOAD_CONCURRENCY, {
            |entry| {
                let tempdir_path = tempdir_path.clone();
                let files = files.clone();
                async move {
                    debug!(path=%entry, "downloading file");
                    let mut stream = storage.get_stream(&entry).await?;
                    let target_path = tempdir_path.join(stream.path.trim_start_matches(prefix));

                    if let Some(parent) = target_path.parent() {
                        fs::create_dir_all(parent).await?;
                    }
                    let mut output_file = fs::File::create(&target_path).await?;
                    io::copy(&mut stream.content, &mut output_file).await?;
                    output_file.sync_all().await?;

                    files.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                }
            }
        })
        .await?;
    let files = files.load(Ordering::Relaxed);

    if files > 0 {
        info!("creating zip file...");
        let (file_list, alg) = retry_async(
            || {
                let path = tempdir.path().to_path_buf();
                async move { storage.store_all_in_archive(target_archive, &path).await }
            },
            3,
        )
        .await?;

        info!("removing temp-dir...");
        fs::remove_dir_all(&tempdir).await?;

        Ok(Some((file_list, alg)))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestEnvironment;
    use docs_rs_storage::{PathNotFoundError, StorageKind, source_archive_path};
    use docs_rs_types::testing::{KRATE, V1};
    use futures_util::StreamExt as _;
    use pretty_assertions::assert_eq;
    use test_case::test_case;

    async fn ls(storage: &AsyncStorage) -> Vec<String> {
        storage
            .list_prefix("")
            .await
            .filter_map(|path| async {
                let Ok(path) = path else { return None };

                if path.starts_with("rustdoc-json/") || path.starts_with("build-logs/") {
                    return None;
                }

                Some(path.clone())
            })
            .collect::<Vec<String>>()
            .await
    }

    #[test_case(StorageKind::S3)]
    #[test_case(StorageKind::Memory)]
    #[tokio::test(flavor = "multi_thread")]
    async fn test_repackage_normal(kind: StorageKind) -> Result<()> {
        let env = TestEnvironment::builder()
            .storage_config(docs_rs_storage::Config::test_config_with_kind(kind)?)
            .build()
            .await?;

        const HTML_PATH: &str = "some/path.html";
        const HTML_CONTENT: &str = "<html>content</html>";
        const SOURCE_PATH: &str = "another/source.rs";
        const SOURCE_CONTENT: &str = "fn main() {}";

        let rid = env
            .fake_release()
            .await
            .name(&KRATE)
            .archive_storage(false)
            .rustdoc_file_with(HTML_PATH, HTML_CONTENT.as_bytes())
            .source_file(SOURCE_PATH, SOURCE_CONTENT.as_bytes())
            .version(V1)
            .create()
            .await?;

        let storage = env.storage()?;

        // confirm we can fetch the files via old file-based storage.
        assert_eq!(
            storage
                .stream_rustdoc_file(&KRATE, &V1, None, HTML_PATH, false)
                .await?
                .materialize(usize::MAX)
                .await?
                .content,
            HTML_CONTENT.as_bytes()
        );

        assert_eq!(
            storage
                .stream_source_file(&KRATE, &V1, None, SOURCE_PATH, false)
                .await?
                .materialize(usize::MAX)
                .await?
                .content,
            SOURCE_CONTENT.as_bytes()
        );

        assert_eq!(
            ls(storage).await,
            vec![
                "rustdoc/krate/1.0.0/krate/index.html",
                "rustdoc/krate/1.0.0/some/path.html",
                "sources/krate/1.0.0/Cargo.toml",
                "sources/krate/1.0.0/another/source.rs",
            ]
        );

        // confirm the target archives really don't exist
        for path in &[
            &rustdoc_archive_path(&KRATE, &V1),
            &source_archive_path(&KRATE, &V1),
        ] {
            assert!(!storage.exists(path).await?);
        }

        let mut conn = env.async_conn().await?;
        repackage(&mut conn, storage, rid, &KRATE, &V1).await?;

        // afterwards it works with rustdoc archives.
        assert_eq!(
            &storage
                .stream_rustdoc_file(&KRATE, &V1, None, HTML_PATH, true)
                .await?
                .materialize(usize::MAX)
                .await?
                .content,
            HTML_CONTENT.as_bytes(),
        );

        // also with source archives.
        assert_eq!(
            &storage
                .stream_source_file(&KRATE, &V1, None, SOURCE_PATH, true)
                .await?
                .materialize(usize::MAX)
                .await?
                .content,
            SOURCE_CONTENT.as_bytes(),
        );

        // all new files are these (`.zip`, `.zip.index`), old files are gone.
        assert_eq!(
            ls(storage).await,
            vec![
                "rustdoc/krate/1.0.0.zip",
                "rustdoc/krate/1.0.0.zip.index",
                "sources/krate/1.0.0.zip",
                "sources/krate/1.0.0.zip.index",
            ]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_repackage_without_rustdoc() -> Result<()> {
        let env = TestEnvironment::builder()
            .storage_config(docs_rs_storage::Config::test_config_with_kind(
                StorageKind::S3,
            )?)
            .build()
            .await?;

        const HTML_PATH: &str = "some/path.html";
        const SOURCE_PATH: &str = "another/source.rs";
        const SOURCE_CONTENT: &str = "fn main() {}";

        let rid = env
            .fake_release()
            .await
            .name(&KRATE)
            .archive_storage(false)
            .rustdoc_file(HTML_PATH) // will be deleted
            .source_file(SOURCE_PATH, SOURCE_CONTENT.as_bytes())
            .version(V1)
            .create()
            .await?;

        let storage = env.storage()?;
        storage
            .delete_prefix(&format!("rustdoc/{KRATE}/{V1}/"))
            .await?;

        // confirm we can fetch the files via old file-based storage.
        assert!(
            !storage
                .rustdoc_file_exists(&KRATE, &V1, None, HTML_PATH, false)
                .await?
        );

        assert_eq!(
            storage
                .stream_source_file(&KRATE, &V1, None, SOURCE_PATH, false)
                .await?
                .materialize(usize::MAX)
                .await?
                .content,
            SOURCE_CONTENT.as_bytes()
        );

        assert_eq!(
            ls(storage).await,
            vec![
                "sources/krate/1.0.0/Cargo.toml",
                "sources/krate/1.0.0/another/source.rs",
            ]
        );

        // confirm the target archives really don't exist
        for path in &[
            &rustdoc_archive_path(&KRATE, &V1),
            &source_archive_path(&KRATE, &V1),
        ] {
            assert!(!storage.exists(path).await?);
        }

        let mut conn = env.async_conn().await?;
        repackage(&mut conn, storage, rid, &KRATE, &V1).await?;

        // but source archive works
        assert_eq!(
            &storage
                .stream_source_file(&KRATE, &V1, None, SOURCE_PATH, true)
                .await?
                .materialize(usize::MAX)
                .await?
                .content,
            SOURCE_CONTENT.as_bytes(),
        );

        // all new files are these (`.zip`, `.zip.index`), old files are gone.
        assert_eq!(
            ls(storage).await,
            vec!["sources/krate/1.0.0.zip", "sources/krate/1.0.0.zip.index",]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_repackage_without_source() -> Result<()> {
        let env = TestEnvironment::builder()
            .storage_config(docs_rs_storage::Config::test_config_with_kind(
                StorageKind::S3,
            )?)
            .build()
            .await?;

        const HTML_PATH: &str = "some/path.html";
        const HTML_CONTENT: &str = "<html>content</html>";
        const SOURCE_PATH: &str = "another/source.rs";
        const SOURCE_CONTENT: &str = "fn main() {}";

        let rid = env
            .fake_release()
            .await
            .name(&KRATE)
            .archive_storage(false)
            .rustdoc_file_with(HTML_PATH, HTML_CONTENT.as_bytes())
            .source_file(SOURCE_PATH, SOURCE_CONTENT.as_bytes())
            .version(V1)
            .create()
            .await?;

        let storage = env.storage()?;
        storage
            .delete_prefix(&format!("sources/{KRATE}/{V1}/"))
            .await?;

        // confirm we can fetch the files via old file-based storage.
        assert_eq!(
            storage
                .stream_rustdoc_file(&KRATE, &V1, None, HTML_PATH, false)
                .await?
                .materialize(usize::MAX)
                .await?
                .content,
            HTML_CONTENT.as_bytes()
        );

        // source file doesn't exist
        assert!(
            storage
                .stream_source_file(&KRATE, &V1, None, SOURCE_PATH, false)
                .await
                .unwrap_err()
                .is::<PathNotFoundError>()
        );

        assert_eq!(
            ls(storage).await,
            vec![
                "rustdoc/krate/1.0.0/krate/index.html",
                "rustdoc/krate/1.0.0/some/path.html",
            ]
        );

        // confirm the target archives really don't exist
        for path in &[
            &rustdoc_archive_path(&KRATE, &V1),
            &source_archive_path(&KRATE, &V1),
        ] {
            assert!(!storage.exists(path).await?);
        }

        let mut conn = env.async_conn().await?;
        repackage(&mut conn, storage, rid, &KRATE, &V1).await?;

        // afterwards it works with rustdoc archives.
        assert_eq!(
            &storage
                .stream_rustdoc_file(&KRATE, &V1, None, HTML_PATH, true)
                .await?
                .materialize(usize::MAX)
                .await?
                .content,
            HTML_CONTENT.as_bytes(),
        );

        // all new files are these (`.zip`, `.zip.index`), old files are gone.
        assert_eq!(
            ls(storage).await,
            vec!["rustdoc/krate/1.0.0.zip", "rustdoc/krate/1.0.0.zip.index",]
        );

        Ok(())
    }
}
