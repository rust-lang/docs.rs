use anyhow::{Context as _, Result};
use docs_rs_context::Context;
use docs_rs_storage::{PathNotFoundError, rustdoc_archive_path, source_archive_path};
use docs_rs_types::{BuildId, KrateName, Version};
use futures_util::TryStreamExt as _;
use tracing::{info, warn};

pub(crate) async fn backfill_file_counts(ctx: &Context) -> Result<(u64, u64, u64, u64)> {
    let mut list_conn = ctx.pool()?.get_async().await?;
    let mut update_conn = ctx.pool()?.get_async().await?;
    let storage = ctx.storage()?;
    let mut sources = 0;
    let mut documentation = 0;
    let mut missing_sources = 0;
    let mut missing_documentation = 0;

    let mut rows = sqlx::query!(
        r#"SELECT
               r.id,
               c.name AS "name: KrateName",
               r.version AS "version: Version",
               r.source_file_count,
               latest_build.id AS "latest_build_id?: BuildId",
               latest_build.documentation_file_count
           FROM releases r
           JOIN crates c ON c.id = r.crate_id
           LEFT JOIN LATERAL (
               SELECT id, documentation_file_count
               FROM builds
               WHERE rid = r.id AND build_status = 'success'
               ORDER BY build_finished DESC, id DESC
               LIMIT 1
           ) latest_build ON true
           WHERE r.source_file_count IS NULL OR (
               latest_build.id IS NOT NULL AND
               latest_build.documentation_file_count IS NULL
           )
           ORDER BY r.id"#,
    )
    .fetch(&mut *list_conn);

    while let Some(row) = rows.try_next().await? {
        info!(%row.name, %row.version, "updating doc & source file counts");

        if row.source_file_count.is_none() {
            let archive = source_archive_path(&row.name, &row.version);
            match storage
                .find_archive_index(&archive, row.latest_build_id)
                .await
            {
                Ok(mut index) => {
                    let count = i64::try_from(index.file_count().await?)?;
                    sources += sqlx::query!(
                        "UPDATE releases
                         SET source_file_count = $2
                         WHERE
                            id = $1 AND
                            source_file_count IS NULL
                        ",
                        row.id,
                        count,
                    )
                    .execute(&mut *update_conn)
                    .await?
                    .rows_affected();
                }
                Err(err) if err.is::<PathNotFoundError>() => {
                    warn!(%archive, "source archive index is missing");
                    missing_sources += 1;
                }
                Err(err) => {
                    return Err(err).with_context(|| format!("fetching index for {archive}"));
                }
            }
        }

        // The archive path is shared by all builds of a release. Match the latest
        // successful build, which is also the cache key used when serving rustdoc.
        if let Some(build_id) = row.latest_build_id
            && row.documentation_file_count.is_none()
        {
            let archive = rustdoc_archive_path(&row.name, &row.version);
            match storage.find_archive_index(&archive, Some(build_id)).await {
                Ok(mut index) => {
                    let count = i64::try_from(index.file_count().await?)?;
                    documentation += sqlx::query!(
                        "UPDATE builds
                         SET documentation_file_count = $2
                         WHERE
                            id = $1 AND
                            documentation_file_count IS NULL
                        ",
                        build_id as _,
                        count,
                    )
                    .execute(&mut *update_conn)
                    .await?
                    .rows_affected();
                }
                Err(err) if err.is::<PathNotFoundError>() => {
                    warn!(%archive, %build_id, "documentation archive index is missing");
                    missing_documentation += 1;
                }
                Err(err) => {
                    return Err(err).with_context(|| format!("fetching index for {archive}"));
                }
            }
        }
    }

    println!(
        "Backfilled {sources} source and {documentation} documentation file counts; \
         skipped {missing_sources} missing source and {missing_documentation} missing documentation indexes"
    );
    Ok((
        sources,
        documentation,
        missing_sources,
        missing_documentation,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestEnvironment;
    use docs_rs_test_fakes::FakeBuild;
    use docs_rs_types::testing::{FOO, V1};

    #[tokio::test(flavor = "multi_thread")]
    async fn backfills_current_archives_without_changing_older_builds() -> Result<()> {
        let env = TestEnvironment::new().await?;
        let release_id = env
            .fake_release()
            .await
            .name(&FOO)
            .version(V1)
            .builds(vec![
                FakeBuild::default(),
                FakeBuild::default(),
                FakeBuild::default().successful(false),
            ])
            .create()
            .await?;
        let mut conn = env.async_conn().await?;

        let builds: Vec<(i32, Option<i64>)> = sqlx::query_as(
            "SELECT id, documentation_file_count FROM builds WHERE rid = $1 ORDER BY id",
        )
        .bind(release_id.0)
        .fetch_all(&mut *conn)
        .await?;
        assert_eq!(builds.len(), 3);
        let source_count = env
            .storage()?
            .find_archive_index(&source_archive_path(&FOO, &V1), Some(BuildId(builds[1].0)))
            .await?
            .file_count()
            .await?;
        let documentation_count = env
            .storage()?
            .find_archive_index(&rustdoc_archive_path(&FOO, &V1), Some(BuildId(builds[1].0)))
            .await?
            .file_count()
            .await?;

        sqlx::query("UPDATE releases SET source_file_count = NULL WHERE id = $1")
            .bind(release_id.0)
            .execute(&mut *conn)
            .await?;
        sqlx::query("UPDATE builds SET documentation_file_count = NULL WHERE rid = $1")
            .bind(release_id.0)
            .execute(&mut *conn)
            .await?;
        sqlx::query("UPDATE builds SET documentation_size = NULL WHERE id = $1")
            .bind(builds[1].0)
            .execute(&mut *conn)
            .await?;
        drop(conn);

        assert_eq!(backfill_file_counts(env.context()).await?, (1, 1, 0, 0));
        assert_eq!(backfill_file_counts(env.context()).await?, (0, 0, 0, 0));

        let mut conn = env.async_conn().await?;
        sqlx::query("UPDATE releases SET source_file_count = NULL WHERE id = $1")
            .bind(release_id.0)
            .execute(&mut *conn)
            .await?;
        drop(conn);
        assert_eq!(backfill_file_counts(env.context()).await?, (1, 0, 0, 0));

        let mut conn = env.async_conn().await?;
        sqlx::query("UPDATE builds SET documentation_file_count = NULL WHERE id = $1")
            .bind(builds[1].0)
            .execute(&mut *conn)
            .await?;
        drop(conn);
        assert_eq!(backfill_file_counts(env.context()).await?, (0, 1, 0, 0));

        let mut conn = env.async_conn().await?;
        let restored_source_count: i64 =
            sqlx::query_scalar("SELECT source_file_count FROM releases WHERE id = $1")
                .bind(release_id.0)
                .fetch_one(&mut *conn)
                .await?;
        let restored_builds: Vec<(i32, Option<i64>)> = sqlx::query_as(
            "SELECT id, documentation_file_count FROM builds WHERE rid = $1 ORDER BY id",
        )
        .bind(release_id.0)
        .fetch_all(&mut *conn)
        .await?;
        assert_eq!(restored_source_count, i64::try_from(source_count)?);
        assert_eq!(restored_builds[0].1, None);
        assert_eq!(
            restored_builds[1].1,
            Some(i64::try_from(documentation_count)?)
        );
        assert_eq!(restored_builds[2].1, None);
        Ok(())
    }
}
