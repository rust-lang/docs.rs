//! Utilities for interacting with the build queue

use crate::error::Result;
use postgres::Client as Connection;

const DEFAULT_PRIORITY: i32 = 0;

/// Get the build queue priority for a crate
pub fn get_crate_priority(conn: &mut Connection, name: &str) -> Result<i32> {
    // Search the `priority` table for a priority where the crate name matches the stored pattern
    let query = conn.query(
        "SELECT priority FROM crate_priorities WHERE $1 LIKE pattern LIMIT 1",
        &[&name],
    )?;

    // If no match is found, return the default priority
    if let Some(row) = query.iter().next() {
        Ok(row.get(0))
    } else {
        Ok(DEFAULT_PRIORITY)
    }
}

/// Set all crates that match [`pattern`] to have a certain priority
///
/// Note: `pattern` is used in a `LIKE` statement, so it must follow the postgres like syntax
///
/// [`pattern`]: https://www.postgresql.org/docs/8.3/functions-matching.html
pub fn set_crate_priority(conn: &mut Connection, pattern: &str, priority: i32) -> Result<()> {
    conn.query(
        "INSERT INTO crate_priorities (pattern, priority) VALUES ($1, $2)",
        &[&pattern, &priority],
    )?;

    Ok(())
}

/// Remove a pattern from the priority table, returning the priority that it was associated with or `None`
/// if nothing was removed
pub fn remove_crate_priority(conn: &mut Connection, pattern: &str) -> Result<Option<i32>> {
    let query = conn.query(
        "DELETE FROM crate_priorities WHERE pattern = $1 RETURNING priority",
        &[&pattern],
    )?;

    Ok(query.iter().next().map(|row| row.get(0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::wrapper;

    #[test]
    fn set_priority() {
        wrapper(|env| {
            let db = env.db();

            set_crate_priority(&db.conn(), "cratesfyi-%", -100)?;
            assert_eq!(get_crate_priority(&db.conn(), "cratesfyi-database")?, -100);
            assert_eq!(get_crate_priority(&db.conn(), "cratesfyi-")?, -100);
            assert_eq!(get_crate_priority(&db.conn(), "cratesfyi-s3")?, -100);
            assert_eq!(get_crate_priority(&db.conn(), "cratesfyi-webserver")?, -100);
            assert_eq!(
                get_crate_priority(&db.conn(), "cratesfyi")?,
                DEFAULT_PRIORITY
            );

            set_crate_priority(&db.conn(), "_c_", 100)?;
            assert_eq!(get_crate_priority(&db.conn(), "rcc")?, 100);
            assert_eq!(get_crate_priority(&db.conn(), "rc")?, DEFAULT_PRIORITY);

            set_crate_priority(&db.conn(), "hexponent", 10)?;
            assert_eq!(get_crate_priority(&db.conn(), "hexponent")?, 10);
            assert_eq!(
                get_crate_priority(&db.conn(), "hexponents")?,
                DEFAULT_PRIORITY
            );
            assert_eq!(
                get_crate_priority(&db.conn(), "floathexponent")?,
                DEFAULT_PRIORITY
            );

            Ok(())
        })
    }

    #[test]
    fn remove_priority() {
        wrapper(|env| {
            let db = env.db();

            set_crate_priority(&db.conn(), "cratesfyi-%", -100)?;
            assert_eq!(get_crate_priority(&db.conn(), "cratesfyi-")?, -100);

            assert_eq!(
                remove_crate_priority(&db.conn(), "cratesfyi-%")?,
                Some(-100)
            );
            assert_eq!(
                get_crate_priority(&db.conn(), "cratesfyi-")?,
                DEFAULT_PRIORITY
            );

            Ok(())
        })
    }

    #[test]
    fn get_priority() {
        wrapper(|env| {
            let db = env.db();

            set_crate_priority(&db.conn(), "cratesfyi-%", -100)?;

            assert_eq!(get_crate_priority(&db.conn(), "cratesfyi-database")?, -100);
            assert_eq!(get_crate_priority(&db.conn(), "cratesfyi-")?, -100);
            assert_eq!(get_crate_priority(&db.conn(), "cratesfyi-s3")?, -100);
            assert_eq!(get_crate_priority(&db.conn(), "cratesfyi-webserver")?, -100);
            assert_eq!(
                get_crate_priority(&db.conn(), "unrelated")?,
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
                get_crate_priority(&db.conn(), "cratesfyi")?,
                DEFAULT_PRIORITY
            );
            assert_eq!(get_crate_priority(&db.conn(), "rcc")?, DEFAULT_PRIORITY);
            assert_eq!(get_crate_priority(&db.conn(), "lasso")?, DEFAULT_PRIORITY);
            assert_eq!(
                get_crate_priority(&db.conn(), "hexponent")?,
                DEFAULT_PRIORITY
            );
            assert_eq!(
                get_crate_priority(&db.conn(), "rust4lyfe")?,
                DEFAULT_PRIORITY
            );

            Ok(())
        })
    }
}
