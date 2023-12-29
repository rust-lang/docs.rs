//! Utilities for interacting with the build queue

use crate::error::Result;
use postgres::Client;

const DEFAULT_PRIORITY: i32 = 0;

/// Get the build queue priority for a crate, returns the matching pattern too
pub fn list_crate_priorities(conn: &mut Client) -> Result<Vec<(String, i32)>> {
    Ok(conn
        .query("SELECT pattern, priority FROM crate_priorities", &[])?
        .into_iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect())
}

/// Get the build queue priority for a crate with its matching pattern
pub fn get_crate_pattern_and_priority(
    conn: &mut Client,
    name: &str,
) -> Result<Option<(String, i32)>> {
    // Search the `priority` table for a priority where the crate name matches the stored pattern
    let query = conn.query(
        "SELECT pattern, priority FROM crate_priorities WHERE $1 LIKE pattern LIMIT 1",
        &[&name],
    )?;

    // If no match is found, return the default priority
    if let Some(row) = query.first() {
        Ok(Some((row.get(0), row.get(1))))
    } else {
        Ok(None)
    }
}

/// Get the build queue priority for a crate
pub fn get_crate_priority(conn: &mut Client, name: &str) -> Result<i32> {
    Ok(get_crate_pattern_and_priority(conn, name)?
        .map_or(DEFAULT_PRIORITY, |(_, priority)| priority))
}

/// Set all crates that match [`pattern`] to have a certain priority
///
/// Note: `pattern` is used in a `LIKE` statement, so it must follow the postgres like syntax
///
/// [`pattern`]: https://www.postgresql.org/docs/8.3/functions-matching.html
pub fn set_crate_priority(conn: &mut Client, pattern: &str, priority: i32) -> Result<()> {
    conn.query(
        "INSERT INTO crate_priorities (pattern, priority) VALUES ($1, $2)",
        &[&pattern, &priority],
    )?;

    Ok(())
}

/// Remove a pattern from the priority table, returning the priority that it was associated with or `None`
/// if nothing was removed
pub fn remove_crate_priority(conn: &mut Client, pattern: &str) -> Result<Option<i32>> {
    let query = conn.query(
        "DELETE FROM crate_priorities WHERE pattern = $1 RETURNING priority",
        &[&pattern],
    )?;

    Ok(query.first().map(|row| row.get(0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::wrapper;

    #[test]
    fn set_priority() {
        wrapper(|env| {
            let db = env.db();

            set_crate_priority(&mut db.conn(), "docsrs-%", -100)?;
            assert_eq!(get_crate_priority(&mut db.conn(), "docsrs-database")?, -100);
            assert_eq!(get_crate_priority(&mut db.conn(), "docsrs-")?, -100);
            assert_eq!(get_crate_priority(&mut db.conn(), "docsrs-s3")?, -100);
            assert_eq!(
                get_crate_priority(&mut db.conn(), "docsrs-webserver")?,
                -100
            );
            assert_eq!(
                get_crate_priority(&mut db.conn(), "docsrs")?,
                DEFAULT_PRIORITY
            );

            set_crate_priority(&mut db.conn(), "_c_", 100)?;
            assert_eq!(get_crate_priority(&mut db.conn(), "rcc")?, 100);
            assert_eq!(get_crate_priority(&mut db.conn(), "rc")?, DEFAULT_PRIORITY);

            set_crate_priority(&mut db.conn(), "hexponent", 10)?;
            assert_eq!(get_crate_priority(&mut db.conn(), "hexponent")?, 10);
            assert_eq!(
                get_crate_priority(&mut db.conn(), "hexponents")?,
                DEFAULT_PRIORITY
            );
            assert_eq!(
                get_crate_priority(&mut db.conn(), "floathexponent")?,
                DEFAULT_PRIORITY
            );

            Ok(())
        })
    }

    #[test]
    fn remove_priority() {
        wrapper(|env| {
            let db = env.db();

            set_crate_priority(&mut db.conn(), "docsrs-%", -100)?;
            assert_eq!(get_crate_priority(&mut db.conn(), "docsrs-")?, -100);

            assert_eq!(
                remove_crate_priority(&mut db.conn(), "docsrs-%")?,
                Some(-100)
            );
            assert_eq!(
                get_crate_priority(&mut db.conn(), "docsrs-")?,
                DEFAULT_PRIORITY
            );

            Ok(())
        })
    }

    #[test]
    fn get_priority() {
        wrapper(|env| {
            let db = env.db();

            set_crate_priority(&mut db.conn(), "docsrs-%", -100)?;

            assert_eq!(get_crate_priority(&mut db.conn(), "docsrs-database")?, -100);
            assert_eq!(get_crate_priority(&mut db.conn(), "docsrs-")?, -100);
            assert_eq!(get_crate_priority(&mut db.conn(), "docsrs-s3")?, -100);
            assert_eq!(
                get_crate_priority(&mut db.conn(), "docsrs-webserver")?,
                -100
            );
            assert_eq!(
                get_crate_priority(&mut db.conn(), "unrelated")?,
                DEFAULT_PRIORITY
            );

            Ok(())
        })
    }

    #[test]
    fn get_default_priority() {
        wrapper(|env| {
            let db = env.db();

            assert_eq!(
                get_crate_priority(&mut db.conn(), "docsrs")?,
                DEFAULT_PRIORITY
            );
            assert_eq!(get_crate_priority(&mut db.conn(), "rcc")?, DEFAULT_PRIORITY);
            assert_eq!(
                get_crate_priority(&mut db.conn(), "lasso")?,
                DEFAULT_PRIORITY
            );
            assert_eq!(
                get_crate_priority(&mut db.conn(), "hexponent")?,
                DEFAULT_PRIORITY
            );
            assert_eq!(
                get_crate_priority(&mut db.conn(), "rust4lyfe")?,
                DEFAULT_PRIORITY
            );

            Ok(())
        })
    }
}
