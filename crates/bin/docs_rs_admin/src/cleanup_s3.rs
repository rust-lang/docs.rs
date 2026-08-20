use anyhow::{Result, bail};
use docs_rs_storage::{AsyncStorage, rustdoc_archive_path, source_archive_path};
use docs_rs_types::{KrateName, Version};
use futures_util::TryStreamExt as _;
use tracing::{info, instrument};

async fn check_archive(storage: &AsyncStorage, path: impl AsRef<str>) -> Result<()> {
    let path = path.as_ref();
    if !storage.exists(path).await? {
        bail!("archive {} missing", path);
    }

    let index_path = format!("{path}.index");
    if !storage.exists(&index_path).await? {
        bail!("archive index {} missing", index_path);
    }

    Ok(())
}

async fn clean_prefix(
    storage: &AsyncStorage,
    prefix: impl AsRef<str>,
    dry_run: bool,
) -> Result<()> {
    let prefix = prefix.as_ref();

    if dry_run {
        let mut stream = storage.list_prefix(prefix).await;

        while let Some(key) = stream.try_next().await? {
            info!(%key, %prefix, "deleting legacy file");
        }
    } else {
        storage.delete_prefix(prefix).await?;
    }

    Ok(())
}

#[instrument(skip_all)]
pub(crate) async fn cleanup_s3_bucket(
    conn: &mut sqlx::PgConnection,
    storage: &AsyncStorage,
    dry_run: bool,
) -> Result<()> {
    let mut result = sqlx::query!(
        r#"SELECT
             c.name as "name: KrateName",
             r.version as "version: Version",
             r.rustdoc_status
          FROM
             crates as c
             INNER JOIN releases AS r ON c.id = r.crate_id
          ORDER BY
             c.name, r.version;
        "#
    )
    .fetch(&mut *conn);

    while let Some(row) = result.try_next().await? {
        info!("checking {} {}", row.name, row.version);

        if row.rustdoc_status.is_some_and(|st| st) {
            check_archive(storage, rustdoc_archive_path(&row.name, &row.version)).await?;
            clean_prefix(
                storage,
                format!("rustdoc/{}/{}/", row.name, row.version),
                dry_run,
            )
            .await?;
        }

        check_archive(storage, source_archive_path(&row.name, &row.version)).await?;

        clean_prefix(
            storage,
            format!("sources/{}/{}/", row.name, row.version),
            dry_run,
        )
        .await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestEnvironment;
    use docs_rs_types::testing::KRATE;
    use pretty_assertions::assert_eq;

    async fn list(storage: &AsyncStorage) -> Result<Vec<String>> {
        let mut result: Vec<_> = storage.list_prefix("").await.try_collect().await?;

        result.sort_unstable();

        Ok(result)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_no_releases_leaves_files() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let storage = env.storage()?;

        storage.store_one("something.html", "content").await?;

        cleanup_s3_bucket(&mut *env.async_conn().await?, storage, false).await?;

        assert_eq!(list(storage).await?, vec!["something.html"]);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_with_releases_nothing_to_delete() -> Result<()> {
        let env = TestEnvironment::new().await?;
        env.fake_release().await.name(&KRATE).create().await?;

        let storage = env.storage()?;
        storage.store_one("something.html", "content").await?;

        let expected_files = vec![
            "build-logs/10000/x86_64-unknown-linux-gnu.txt",
            "rustdoc-json/krate/1.0.0/x86_64-unknown-linux-gnu/krate_1.0.0_x86_64-unknown-linux-gnu_42.json.gz",
            "rustdoc-json/krate/1.0.0/x86_64-unknown-linux-gnu/krate_1.0.0_x86_64-unknown-linux-gnu_42.json.zst",
            "rustdoc-json/krate/1.0.0/x86_64-unknown-linux-gnu/krate_1.0.0_x86_64-unknown-linux-gnu_latest.json.gz",
            "rustdoc-json/krate/1.0.0/x86_64-unknown-linux-gnu/krate_1.0.0_x86_64-unknown-linux-gnu_latest.json.zst",
            "rustdoc/krate/1.0.0.zip",
            "rustdoc/krate/1.0.0.zip.index",
            "something.html",
            "sources/krate/1.0.0.zip",
            "sources/krate/1.0.0.zip.index",
        ];

        assert_eq!(list(storage).await?, expected_files);

        cleanup_s3_bucket(&mut *env.async_conn().await?, storage, false).await?;

        assert_eq!(list(storage).await?, expected_files);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_with_releases_and_legacy_files() -> Result<()> {
        let env = TestEnvironment::new().await?;
        env.fake_release().await.name(&KRATE).create().await?;

        let storage = env.storage()?;
        storage.store_one("something.html", "content").await?;
        storage
            .store_one("rustdoc/krate/1.0.0/legacy/1.html", "content")
            .await?;
        storage
            .store_one("rustdoc/krate/1.0.0/2.html", "content")
            .await?;
        storage
            .store_one("rustdoc/krate/left_alone.html", "content")
            .await?;
        storage
            .store_one("sources/krate/left_alone.html", "content")
            .await?;
        storage
            .store_one("sources/krate/1.0.0/legacy/1.rs", "content")
            .await?;
        storage
            .store_one("sources/krate/1.0.0/2.rs", "content")
            .await?;

        let old_count = list(storage).await?.len();

        // dry-run does nothing
        cleanup_s3_bucket(&mut *env.async_conn().await?, storage, true).await?;
        assert_eq!(list(storage).await?.len(), old_count);

        // real clean does clean
        cleanup_s3_bucket(&mut *env.async_conn().await?, storage, false).await?;

        assert_eq!(
            list(storage).await?,
            vec![
                "build-logs/10000/x86_64-unknown-linux-gnu.txt",
                "rustdoc-json/krate/1.0.0/x86_64-unknown-linux-gnu/krate_1.0.0_x86_64-unknown-linux-gnu_42.json.gz",
                "rustdoc-json/krate/1.0.0/x86_64-unknown-linux-gnu/krate_1.0.0_x86_64-unknown-linux-gnu_42.json.zst",
                "rustdoc-json/krate/1.0.0/x86_64-unknown-linux-gnu/krate_1.0.0_x86_64-unknown-linux-gnu_latest.json.gz",
                "rustdoc-json/krate/1.0.0/x86_64-unknown-linux-gnu/krate_1.0.0_x86_64-unknown-linux-gnu_latest.json.zst",
                "rustdoc/krate/1.0.0.zip",
                "rustdoc/krate/1.0.0.zip.index",
                "rustdoc/krate/left_alone.html",
                "something.html",
                "sources/krate/1.0.0.zip",
                "sources/krate/1.0.0.zip.index",
                "sources/krate/left_alone.html",
            ]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_failed_build() -> Result<()> {
        let env = TestEnvironment::new().await?;
        env.fake_release()
            .await
            .name(&KRATE)
            .build_result_failed() // sets `rustdoc_status = false`
            .create()
            .await?;

        let storage = env.storage()?;
        storage.store_one("something.html", "content").await?;
        storage
            .store_one("sources/krate/left_alone.html", "content")
            .await?;
        storage
            .store_one("sources/krate/1.0.0/legacy/1.rs", "content")
            .await?;
        storage
            .store_one("sources/krate/1.0.0/2.rs", "content")
            .await?;

        let old_count = list(storage).await?.len();

        // dry-run does nothing
        cleanup_s3_bucket(&mut *env.async_conn().await?, storage, true).await?;
        assert_eq!(list(storage).await?.len(), old_count);

        // real clean does clean
        cleanup_s3_bucket(&mut *env.async_conn().await?, storage, false).await?;

        assert_eq!(
            list(storage).await?,
            vec![
                "build-logs/10000/x86_64-unknown-linux-gnu.txt",
                "something.html",
                "sources/krate/1.0.0.zip",
                "sources/krate/1.0.0.zip.index",
                "sources/krate/left_alone.html",
            ]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_binary_crate() -> Result<()> {
        let env = TestEnvironment::new().await?;
        env.fake_release()
            .await
            .name(&KRATE)
            .binary(true) // sets `rustdoc_status = false`
            .create()
            .await?;

        let storage = env.storage()?;
        storage.store_one("something.html", "content").await?;
        storage
            .store_one("sources/krate/left_alone.html", "content")
            .await?;
        storage
            .store_one("sources/krate/1.0.0/legacy/1.rs", "content")
            .await?;
        storage
            .store_one("sources/krate/1.0.0/2.rs", "content")
            .await?;

        let old_count = list(storage).await?.len();

        // dry-run does nothing
        cleanup_s3_bucket(&mut *env.async_conn().await?, storage, true).await?;
        assert_eq!(list(storage).await?.len(), old_count);

        // real clean does clean
        cleanup_s3_bucket(&mut *env.async_conn().await?, storage, false).await?;

        assert_eq!(
            list(storage).await?,
            vec![
                "build-logs/10000/x86_64-unknown-linux-gnu.txt",
                "something.html",
                "sources/krate/1.0.0.zip",
                "sources/krate/1.0.0.zip.index",
                "sources/krate/left_alone.html",
            ]
        );

        Ok(())
    }
}
