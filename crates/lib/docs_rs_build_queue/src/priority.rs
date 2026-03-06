use crate::PRIORITY_DEFAULT;
use anyhow::Result;
use docs_rs_types::KrateName;
use futures_util::stream::TryStreamExt;

/// Get the build queue priority for a crate, returns the matching pattern too
pub async fn list_crate_priorities(conn: &mut sqlx::PgConnection) -> Result<Vec<(String, i32)>> {
    Ok(
        sqlx::query!("SELECT pattern, priority FROM crate_priorities")
            .fetch(conn)
            .map_ok(|r| (r.pattern, r.priority))
            .try_collect()
            .await?,
    )
}

/// Get the build queue priority for a crate with its matching pattern
pub async fn get_crate_pattern_and_priority(
    conn: &mut sqlx::PgConnection,
    name: &KrateName,
) -> Result<Option<(String, i32)>> {
    // Search the `priority` table for a priority where the crate name matches the stored pattern
    Ok(sqlx::query!(
        "SELECT pattern, priority FROM crate_priorities WHERE $1 LIKE pattern LIMIT 1",
        name as _
    )
    .fetch_optional(&mut *conn)
    .await?
    .map(|row| (row.pattern, row.priority)))
}

/// Get the build queue priority for a crate
pub async fn get_crate_priority(conn: &mut sqlx::PgConnection, name: &KrateName) -> Result<i32> {
    Ok(get_crate_pattern_and_priority(conn, name)
        .await?
        .map_or(PRIORITY_DEFAULT, |(_, priority)| priority))
}

/// Set all crates that match [`pattern`] to have a certain priority
///
/// Note: `pattern` is used in a `LIKE` statement, so it must follow the postgres like syntax
///
/// [`pattern`]: https://www.postgresql.org/docs/8.3/functions-matching.html
pub async fn set_crate_priority(
    conn: &mut sqlx::PgConnection,
    pattern: &str,
    priority: i32,
) -> Result<()> {
    sqlx::query!(
        "INSERT INTO crate_priorities (pattern, priority) VALUES ($1, $2)",
        pattern,
        priority,
    )
    .execute(&mut *conn)
    .await?;

    Ok(())
}

/// Remove a pattern from the priority table, returning the priority that it was associated with or `None`
/// if nothing was removed
pub async fn remove_crate_priority(
    conn: &mut sqlx::PgConnection,
    pattern: &str,
) -> Result<Option<i32>> {
    Ok(sqlx::query_scalar!(
        "DELETE FROM crate_priorities WHERE pattern = $1 RETURNING priority",
        pattern,
    )
    .fetch_optional(&mut *conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use docs_rs_config::AppConfig as _;
    use docs_rs_database::{Config, testing::TestDatabase};
    use docs_rs_opentelemetry::testing::TestMetrics;
    use test_case::test_case;

    #[test_case(
        "docsrs-%",
        &["docsrs-database", "docsrs-", "docsrs-s3", "docsrs-webserver"],
        &["docsrs"]
    )]
    #[test_case(
        "_c_",
        &["rcc"],
        &["rc"]
    )]
    #[test_case(
        "hexponent",
        &["hexponent"],
        &["hexponents", "floathexponent"]
    )]
    #[tokio::test(flavor = "multi_thread")]
    async fn set_priority(
        pattern: &str,
        should_match: &[&str],
        should_not_match: &[&str],
    ) -> Result<()> {
        let test_metrics = TestMetrics::new();
        let db = TestDatabase::new(&Config::test_config()?, test_metrics.provider()).await?;

        const PRIO: i32 = -100;

        let mut conn = db.async_conn().await?;

        set_crate_priority(&mut conn, pattern, PRIO).await?;

        for name in should_match {
            assert_eq!(
                get_crate_priority(&mut conn, &name.parse().unwrap()).await?,
                PRIO
            );
        }

        for name in should_not_match {
            assert_eq!(
                get_crate_priority(&mut conn, &name.parse().unwrap()).await?,
                PRIORITY_DEFAULT
            );
        }

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn remove_priority() -> Result<()> {
        let test_metrics = TestMetrics::new();
        let db = TestDatabase::new(&Config::test_config()?, test_metrics.provider()).await?;

        let mut conn = db.async_conn().await?;
        let pattern = "docsrs-%";
        let krate = KrateName::from_static("docsrs-");
        const PRIO: i32 = -100;

        set_crate_priority(&mut conn, pattern, PRIO).await?;
        assert_eq!(get_crate_priority(&mut conn, &krate).await?, PRIO);

        assert_eq!(remove_crate_priority(&mut conn, pattern).await?, Some(PRIO));
        assert_eq!(
            get_crate_priority(&mut conn, &krate).await?,
            PRIORITY_DEFAULT
        );

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn get_default_priority() -> Result<()> {
        let test_metrics = TestMetrics::new();
        let db = TestDatabase::new(&Config::test_config()?, test_metrics.provider()).await?;

        let mut conn = db.async_conn().await?;

        for name in &["docsrs", "rcc", "lasso", "hexponent", "rust4lyfe"] {
            let krate = KrateName::from_static(name);

            assert_eq!(
                get_crate_priority(&mut conn, &krate).await?,
                PRIORITY_DEFAULT
            );
        }

        Ok(())
    }
}
