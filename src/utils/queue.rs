//! Utilities for interacting with the build queue

use crate::error::Result;
use postgres::Connection;

const DEFAULT_PRIORITY: i32 = 0;

/// Get the build queue priority for a crate
pub fn get_crate_priority(conn: &Connection, name: &str) -> Result<i32> {
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

/// Adds a crate to the build queue to be built by rustdoc. `priority` should be gotten from `get_crate_priority`
pub fn add_crate_to_queue(
    conn: &Connection,
    name: &str,
    version: &str,
    priority: i32,
) -> Result<()> {
    conn.execute(
        "INSERT INTO queue (name, version, priority) VALUES ($1, $2, $3)",
        &[&name, &version, &priority],
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::wrapper;

    /// Set all crates that match [`pattern`] to have a certain priority
    ///
    /// Note: `pattern` is used in a `LIKE` statement, so it must follow the postgres like syntax
    ///
    /// [`pattern`]: https://www.postgresql.org/docs/8.3/functions-matching.html
    pub fn set_crate_priority(conn: &Connection, pattern: &str, priority: i32) -> Result<()> {
        conn.query(
            "INSERT INTO crate_priorities (pattern, priority) VALUES ($1, $2)",
            &[&pattern, &priority],
        )?;

        Ok(())
    }

    /// Remove a pattern from the priority table, returning the priority that it was associated with or `None`
    /// if nothing was removed
    pub fn remove_crate_priority(conn: &Connection, pattern: &str) -> Result<Option<i32>> {
        let query = conn.query(
            "DELETE FROM crate_priorities WHERE pattern = $1 RETURNING priority",
            &[&pattern],
        )?;

        Ok(query.iter().next().map(|row| row.get(0)))
    }

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

    #[test]
    fn add_to_queue() {
        wrapper(|env| {
            let db = env.db();

            let test_crates = [
                ("rcc", "0.1.0", 2),
                ("lasso", "0.1.0", -1),
                ("hexponent", "0.1.0", 0),
                ("destroy-all-humans", "0.0.0-alpha", -100000),
                ("totally-not-destroying-humans", "0.0.1", 0),
            ];

            for (name, version, priority) in test_crates.iter() {
                add_crate_to_queue(&db.conn(), name, version, *priority)?;

                let query = db.conn().query(
                    "SELECT name, version, priority FROM queue WHERE name = $1",
                    &[&name],
                )?;

                assert!(query.len() == 1);
                let row = query.iter().next().unwrap();

                assert_eq!(&row.get::<_, String>(0), name);
                assert_eq!(&row.get::<_, String>(1), version);
                assert_eq!(row.get::<_, i32>(2), *priority);
            }

            Ok(())
        })
    }
}
