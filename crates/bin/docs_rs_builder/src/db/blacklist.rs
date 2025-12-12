use anyhow::Result;
use futures_util::stream::TryStreamExt;

#[derive(Debug, thiserror::Error)]
enum BlacklistError {
    #[error("crate {0} is already on the blacklist")]
    CrateAlreadyOnBlacklist(String),

    #[error("crate {0} is not on the blacklist")]
    CrateNotOnBlacklist(String),
}

/// Returns whether the given name is blacklisted.
pub async fn is_blacklisted(conn: &mut sqlx::PgConnection, name: &str) -> Result<bool> {
    Ok(sqlx::query_scalar!(
        r#"SELECT COUNT(*) as "count!" FROM blacklisted_crates WHERE crate_name = $1;"#,
        name
    )
    .fetch_one(conn)
    .await?
        != 0)
}

/// Returns the crate names on the blacklist, sorted ascending.
pub async fn list_crates(conn: &mut sqlx::PgConnection) -> Result<Vec<String>> {
    Ok(
        sqlx::query!("SELECT crate_name FROM blacklisted_crates ORDER BY crate_name asc;")
            .fetch(conn)
            .map_ok(|row| row.crate_name)
            .try_collect()
            .await?,
    )
}

/// Adds a crate to the blacklist.
pub async fn add_crate(conn: &mut sqlx::PgConnection, name: &str) -> Result<()> {
    if is_blacklisted(&mut *conn, name).await? {
        return Err(BlacklistError::CrateAlreadyOnBlacklist(name.into()).into());
    }

    sqlx::query!(
        "INSERT INTO blacklisted_crates (crate_name) VALUES ($1);",
        name
    )
    .execute(conn)
    .await?;

    Ok(())
}

/// Removes a crate from the blacklist.
pub async fn remove_crate(conn: &mut sqlx::PgConnection, name: &str) -> Result<()> {
    if !is_blacklisted(conn, name).await? {
        return Err(BlacklistError::CrateNotOnBlacklist(name.into()).into());
    }

    sqlx::query!(
        "DELETE FROM blacklisted_crates WHERE crate_name = $1;",
        name
    )
    .execute(conn)
    .await?;

    Ok(())
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     #[test]
//     fn test_list_blacklist() {
//         crate::test::async_wrapper(|env| async move {
//             let mut conn = env.async_db().async_conn().await;

//             // crates are added out of order to verify sorting
//             add_crate(&mut conn, "crate A").await?;
//             add_crate(&mut conn, "crate C").await?;
//             add_crate(&mut conn, "crate B").await?;

//             assert!(list_crates(&mut conn).await? == vec!["crate A", "crate B", "crate C"]);
//             Ok(())
//         });
//     }

//     #[test]
//     fn test_add_to_and_remove_from_blacklist() {
//         crate::test::async_wrapper(|env| async move {
//             let mut conn = env.async_db().async_conn().await;

//             assert!(!is_blacklisted(&mut conn, "crate foo").await?);
//             add_crate(&mut conn, "crate foo").await?;
//             assert!(is_blacklisted(&mut conn, "crate foo").await?);
//             remove_crate(&mut conn, "crate foo").await?;
//             assert!(!is_blacklisted(&mut conn, "crate foo").await?);
//             Ok(())
//         });
//     }

//     #[test]
//     fn test_add_twice_to_blacklist() {
//         crate::test::async_wrapper(|env| async move {
//             let mut conn = env.async_db().async_conn().await;

//             add_crate(&mut conn, "crate foo").await?;
//             assert!(add_crate(&mut conn, "crate foo").await.is_err());
//             add_crate(&mut conn, "crate bar").await?;

//             Ok(())
//         });
//     }

//     #[test]
//     fn test_remove_non_existing_crate() {
//         crate::test::async_wrapper(|env| async move {
//             let mut conn = env.async_db().async_conn().await;

//             assert!(remove_crate(&mut conn, "crate foo").await.is_err());

//             Ok(())
//         });
//     }
// }
