use std::collections::HashMap;

use anyhow::Result;
use docs_rs_types::KrateName;
use sqlx::Acquire as _;

/// update the crate-count on each repository.
///
/// Assuming we can find workspaces by finding crates with the same
/// repository, we could
pub async fn update_repository_stats(
    conn: &mut sqlx::PgConnection,
    repository_id: i32,
) -> Result<()> {
    sqlx::query!(
        r#"
        UPDATE repositories as repo
        SET crate_count = (
            select count(*)
            from releases AS r
            inner join crates as c on c.latest_version_id = r.id
            where r.repository_id = repo.id
        )
        WHERE id = $1
        "#,
        repository_id
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// rewite the crate-count on each repository.
///
/// At the time of writing (2026-02-08, 100k repositories), takes ~10s
pub async fn rewrite_repository_stats(conn: &mut sqlx::PgConnection) -> Result<()> {
    let mut transaction = conn.begin().await?;

    sqlx::query!("UPDATE repositories SET crate_count = 0")
        .execute(&mut *transaction)
        .await?;

    sqlx::query!(
        r#"
        WITH counts AS (
            SELECT r.repository_id, count(*) AS crate_count
            FROM releases AS r
            JOIN crates AS c ON c.latest_version_id = r.id
            WHERE r.repository_id IS NOT NULL
            GROUP BY r.repository_id
        )

        UPDATE repositories AS repo
        SET crate_count = coalesce(counts.crate_count, 0)
        FROM counts
        WHERE
            repo.id = counts.repository_id
            AND repo.crate_count IS DISTINCT FROM counts.crate_count
        "#
    )
    .execute(&mut *transaction)
    .await?;

    transaction.commit().await?;

    Ok(())
}

/// get the crate-count for the related workspace for a crate.
pub async fn get_crate_counts(
    conn: &mut sqlx::PgConnection,
    names: impl IntoIterator<Item = KrateName>,
) -> Result<HashMap<KrateName, i32>> {
    let names: Vec<_> = names
        .into_iter()
        .map(|k: KrateName| k.to_string())
        .collect();

    Ok(sqlx::query!(
        r#"
        SELECT
            c.name as "name: KrateName",
            repo.crate_count

        FROM
            crates AS c
            INNER JOIN releases AS r ON c.latest_version_id = r.id
            INNER JOIN repositories AS repo ON r.repository_id = repo.id

        WHERE c.name = ANY($1)
        "#,
        &names[..],
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|row| (row.name, row.crate_count))
    .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use docs_rs_config::AppConfig as _;
    use docs_rs_database::testing::TestDatabase;
    use docs_rs_opentelemetry::testing::TestMetrics;
    use docs_rs_storage::testing::TestStorage;
    use docs_rs_test_fakes::{FakeGithubStats, FakeRelease};
    use docs_rs_types::testing::{BAR, BAZ, FOO};
    use pretty_assertions::assert_eq;

    struct TestEnv {
        db: TestDatabase,
        storage: TestStorage,
    }

    impl TestEnv {
        async fn fake_release(&self) -> FakeRelease<'_> {
            FakeRelease::new(self.db.pool().clone(), self.storage.storage().clone())
        }
    }

    async fn test_env() -> Result<TestEnv> {
        let test_metrics = TestMetrics::new();
        let db = TestDatabase::new(
            &docs_rs_database::Config::test_config()?,
            test_metrics.provider(),
        )
        .await?;

        let storage = TestStorage::from_config(
            docs_rs_storage::Config::test_config()?.into(),
            test_metrics.provider(),
        )
        .await?;

        Ok(TestEnv { db, storage })
    }

    async fn fetch_stats(conn: &mut sqlx::PgConnection) -> Result<Vec<(String, i32)>> {
        Ok(
            sqlx::query!("SELECT name, crate_count FROM repositories ORDER BY name")
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .map(|row| (row.name, row.crate_count))
                .collect::<Vec<_>>(),
        )
    }

    async fn fetch_repo_id(conn: &mut sqlx::PgConnection, name: &str) -> Result<i32> {
        Ok(
            sqlx::query_scalar!("SELECT id FROM repositories WHERE name = $1", name)
                .fetch_one(&mut *conn)
                .await?,
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_update_repository_stats_single() -> Result<()> {
        let env = test_env().await?;

        env.fake_release()
            .await
            .name(&FOO)
            .github_stats("owner1/repo1", 0, 0, 0)
            .create()
            .await?;

        env.fake_release()
            .await
            .name(&BAR)
            .github_stats("owner2/repo2", 0, 0, 0)
            .create()
            .await?;

        let mut conn = env.db.async_conn().await?;

        // initial result, zero everything, because neither the full rewrite or the single-repo
        // update was called
        assert_eq!(
            fetch_stats(&mut conn).await?,
            vec![("owner1/repo1".into(), 0), ("owner2/repo2".into(), 0)]
        );

        rewrite_repository_stats(&mut conn).await?;

        // after the full rewrite, the count is correct
        assert_eq!(
            fetch_stats(&mut conn).await?,
            vec![("owner1/repo1".into(), 1), ("owner2/repo2".into(), 1)]
        );

        env.fake_release()
            .await
            .name(&BAZ)
            .github_stats("owner3/repo3", 0, 0, 0)
            .create()
            .await?;

        // after adding a release, the count is still 1 for old repos, the new is zero,
        // because neither the full rewrite or the single-repo update was called
        assert_eq!(
            fetch_stats(&mut conn).await?,
            vec![
                ("owner1/repo1".into(), 1),
                ("owner2/repo2".into(), 1),
                ("owner3/repo3".into(), 0),
            ]
        );

        let repo3_id = fetch_repo_id(&mut conn, "owner3/repo3").await?;

        update_repository_stats(&mut conn, repo3_id).await?;

        // after calling the single-repo update, the count for repo3 is correct,
        // and the old repos are unchanged
        assert_eq!(
            fetch_stats(&mut conn).await?,
            vec![
                ("owner1/repo1".into(), 1),
                ("owner2/repo2".into(), 1),
                ("owner3/repo3".into(), 1),
            ]
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_update_repository_stats_multi() -> Result<()> {
        let env = test_env().await?;
        let mut conn = env.db.async_conn().await?;

        let repo_id = FakeGithubStats {
            repo: "owner/repo".into(),
            stars: 0,
            forks: 0,
            issues: 0,
        }
        .create(&mut conn)
        .await?;

        for name in &[FOO, BAR] {
            env.fake_release().await.name(name).create().await?;
        }

        sqlx::query!("UPDATE releases SET repository_id = $1", repo_id)
            .execute(&mut *conn)
            .await?;

        // the stats should be 0, because neither the full rewrite or the single-repo update was
        // called
        assert_eq!(
            fetch_stats(&mut conn).await?,
            vec![("owner/repo".into(), 0)]
        );

        rewrite_repository_stats(&mut conn).await?;

        // after the full rewrite, the count is correct
        assert_eq!(
            fetch_stats(&mut conn).await?,
            vec![("owner/repo".into(), 2)]
        );

        env.fake_release().await.name(&BAZ).create().await?;
        sqlx::query!("UPDATE releases SET repository_id = $1", repo_id)
            .execute(&mut *conn)
            .await?;

        // after adding a release, the count is still 2,
        // because neither the full rewrite or the single-repo update was called
        assert_eq!(
            fetch_stats(&mut conn).await?,
            vec![("owner/repo".into(), 2)]
        );

        update_repository_stats(&mut conn, repo_id).await?;

        // here we expect the count to be 3, because we called the single-repo update
        assert_eq!(
            fetch_stats(&mut conn).await?,
            vec![("owner/repo".into(), 3),]
        );

        Ok(())
    }
}
