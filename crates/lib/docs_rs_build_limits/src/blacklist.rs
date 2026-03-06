use docs_rs_types::KrateName;
use futures_util::stream::TryStreamExt;

type Result<T> = std::result::Result<T, BlacklistError>;

#[derive(Debug, thiserror::Error)]
pub enum BlacklistError {
    #[error("crate {0} is already on the blacklist")]
    CrateAlreadyOnBlacklist(KrateName),

    #[error("crate {0} is not on the blacklist")]
    CrateNotOnBlacklist(KrateName),

    #[error(transparent)]
    DatabaseError(#[from] sqlx::Error),
}

/// Returns whether the given name is blacklisted.
pub async fn is_blacklisted(conn: &mut sqlx::PgConnection, name: &KrateName) -> Result<bool> {
    Ok(sqlx::query_scalar!(
        r#"SELECT 1  FROM blacklisted_crates WHERE crate_name = $1;"#,
        name as _
    )
    .fetch_optional(conn)
    .await?
    .is_some())
}

/// Returns the crate names on the blacklist, sorted ascending.
pub async fn list_crates(conn: &mut sqlx::PgConnection) -> Result<Vec<KrateName>> {
    Ok(sqlx::query!(
        r#"
            SELECT
                crate_name as "crate_name: KrateName"
            FROM blacklisted_crates
            ORDER BY crate_name asc;
        "#
    )
    .fetch(conn)
    .map_ok(|row| row.crate_name)
    .try_collect()
    .await?)
}

/// Adds a crate to the blacklist.
pub async fn add_crate(conn: &mut sqlx::PgConnection, name: &KrateName) -> Result<()> {
    if is_blacklisted(&mut *conn, name).await? {
        return Err(BlacklistError::CrateAlreadyOnBlacklist(name.into()));
    }

    sqlx::query!(
        "INSERT INTO blacklisted_crates (crate_name) VALUES ($1);",
        name as _
    )
    .execute(conn)
    .await?;

    Ok(())
}

/// Removes a crate from the blacklist.
pub async fn remove_crate(conn: &mut sqlx::PgConnection, name: &KrateName) -> Result<()> {
    if !is_blacklisted(conn, name).await? {
        return Err(BlacklistError::CrateNotOnBlacklist(name.into()));
    }

    sqlx::query!(
        "DELETE FROM blacklisted_crates WHERE crate_name = $1;",
        name as _
    )
    .execute(conn)
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use docs_rs_config::AppConfig as _;
    use docs_rs_database::testing::TestDatabase;
    use docs_rs_opentelemetry::testing::TestMetrics;
    use docs_rs_types::testing::{BAR, BAZ, FOO};
    use pretty_assertions::assert_eq;

    async fn test_db() -> anyhow::Result<TestDatabase> {
        let test_metrics = TestMetrics::new();
        let db = TestDatabase::new(
            &docs_rs_database::Config::test_config()?,
            test_metrics.provider(),
        )
        .await?;
        Ok(db)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_list_blacklist() -> Result<()> {
        let db = test_db().await?;
        let mut conn = db.async_conn().await?;

        // crates are added out of order to verify sorting
        add_crate(&mut conn, &FOO).await?;
        add_crate(&mut conn, &BAR).await?;
        add_crate(&mut conn, &BAZ).await?;

        assert_eq!(list_crates(&mut conn).await?, vec![BAR, BAZ, FOO]);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_add_to_and_remove_from_blacklist() -> Result<()> {
        let db = test_db().await?;
        let mut conn = db.async_conn().await?;

        assert!(!is_blacklisted(&mut conn, &FOO).await?);
        add_crate(&mut conn, &FOO).await?;
        assert!(is_blacklisted(&mut conn, &FOO).await?);
        remove_crate(&mut conn, &FOO).await?;
        assert!(!is_blacklisted(&mut conn, &FOO).await?);
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_add_twice_to_blacklist() -> Result<()> {
        let db = test_db().await?;
        let mut conn = db.async_conn().await?;

        add_crate(&mut conn, &FOO).await?;
        assert!(add_crate(&mut conn, &FOO).await.is_err());
        add_crate(&mut conn, &BAR).await?;

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_remove_non_existing_crate() -> Result<()> {
        let db = test_db().await?;
        let mut conn = db.async_conn().await?;

        assert!(remove_crate(&mut conn, &FOO).await.is_err());

        Ok(())
    }
}
