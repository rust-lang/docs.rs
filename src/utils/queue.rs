//! Utilities for interacting with the build queue
use crate::error::Result;
use docs_rs_build_queue::PRIORITY_DEFAULT;
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
    name: &str,
) -> Result<Option<(String, i32)>> {
    // Search the `priority` table for a priority where the crate name matches the stored pattern
    Ok(sqlx::query!(
        "SELECT pattern, priority FROM crate_priorities WHERE $1 LIKE pattern LIMIT 1",
        name
    )
    .fetch_optional(&mut *conn)
    .await?
    .map(|row| (row.pattern, row.priority)))
}

/// Get the build queue priority for a crate
pub async fn get_crate_priority(conn: &mut sqlx::PgConnection, name: &str) -> Result<i32> {
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
    use crate::test::async_wrapper;

    #[test]
    fn set_priority() {
        async_wrapper(|env| async move {
            let db = env.async_db();
            let mut conn = db.async_conn().await;

            set_crate_priority(&mut conn, "docsrs-%", -100).await?;
            assert_eq!(
                get_crate_priority(&mut conn, "docsrs-database").await?,
                -100
            );
            assert_eq!(get_crate_priority(&mut conn, "docsrs-").await?, -100);
            assert_eq!(get_crate_priority(&mut conn, "docsrs-s3").await?, -100);
            assert_eq!(
                get_crate_priority(&mut conn, "docsrs-webserver").await?,
                -100
            );
            assert_eq!(
                get_crate_priority(&mut conn, "docsrs").await?,
                PRIORITY_DEFAULT
            );

            set_crate_priority(&mut conn, "_c_", 100).await?;
            assert_eq!(get_crate_priority(&mut conn, "rcc").await?, 100);
            assert_eq!(get_crate_priority(&mut conn, "rc").await?, PRIORITY_DEFAULT);

            set_crate_priority(&mut conn, "hexponent", 10).await?;
            assert_eq!(get_crate_priority(&mut conn, "hexponent").await?, 10);
            assert_eq!(
                get_crate_priority(&mut conn, "hexponents").await?,
                PRIORITY_DEFAULT
            );
            assert_eq!(
                get_crate_priority(&mut conn, "floathexponent").await?,
                PRIORITY_DEFAULT
            );

            Ok(())
        })
    }

    #[test]
    fn remove_priority() {
        async_wrapper(|env| async move {
            let db = env.async_db();
            let mut conn = db.async_conn().await;

            set_crate_priority(&mut conn, "docsrs-%", -100).await?;
            assert_eq!(get_crate_priority(&mut conn, "docsrs-").await?, -100);

            assert_eq!(
                remove_crate_priority(&mut conn, "docsrs-%").await?,
                Some(-100)
            );
            assert_eq!(
                get_crate_priority(&mut conn, "docsrs-").await?,
                PRIORITY_DEFAULT
            );

            Ok(())
        })
    }

    #[test]
    fn get_priority() {
        async_wrapper(|env| async move {
            let db = env.async_db();
            let mut conn = db.async_conn().await;

            set_crate_priority(&mut conn, "docsrs-%", -100).await?;

            assert_eq!(
                get_crate_priority(&mut conn, "docsrs-database").await?,
                -100
            );
            assert_eq!(get_crate_priority(&mut conn, "docsrs-").await?, -100);
            assert_eq!(get_crate_priority(&mut conn, "docsrs-s3").await?, -100);
            assert_eq!(
                get_crate_priority(&mut conn, "docsrs-webserver").await?,
                -100
            );
            assert_eq!(
                get_crate_priority(&mut conn, "unrelated").await?,
                PRIORITY_DEFAULT
            );

            Ok(())
        })
    }

    #[test]
    fn get_default_priority() {
        async_wrapper(|env| async move {
            let db = env.async_db();
            let mut conn = db.async_conn().await;

            assert_eq!(
                get_crate_priority(&mut conn, "docsrs").await?,
                PRIORITY_DEFAULT
            );
            assert_eq!(
                get_crate_priority(&mut conn, "rcc").await?,
                PRIORITY_DEFAULT
            );
            assert_eq!(
                get_crate_priority(&mut conn, "lasso").await?,
                PRIORITY_DEFAULT
            );
            assert_eq!(
                get_crate_priority(&mut conn, "hexponent").await?,
                PRIORITY_DEFAULT
            );
            assert_eq!(
                get_crate_priority(&mut conn, "rust4lyfe").await?,
                PRIORITY_DEFAULT
            );

            Ok(())
        })
    }
}
