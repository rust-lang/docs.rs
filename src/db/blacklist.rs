use crate::error::Result;
use postgres::Client;

#[derive(Debug, thiserror::Error)]
enum BlacklistError {
    #[error("crate {0} is already on the blacklist")]
    CrateAlreadyOnBlacklist(String),

    #[error("crate {0} is not on the blacklist")]
    CrateNotOnBlacklist(String),
}

/// Returns whether the given name is blacklisted.
pub fn is_blacklisted(conn: &mut Client, name: &str) -> Result<bool> {
    let rows = conn.query(
        "SELECT COUNT(*) FROM blacklisted_crates WHERE crate_name = $1;",
        &[&name],
    )?;
    let count: i64 = rows[0].get(0);

    Ok(count != 0)
}

/// Returns the crate names on the blacklist, sorted ascending.
pub fn list_crates(conn: &mut Client) -> Result<Vec<String>> {
    let rows = conn.query(
        "SELECT crate_name FROM blacklisted_crates ORDER BY crate_name asc;",
        &[],
    )?;

    Ok(rows.into_iter().map(|row| row.get(0)).collect())
}

/// Adds a crate to the blacklist.
pub fn add_crate(conn: &mut Client, name: &str) -> Result<()> {
    if is_blacklisted(conn, name)? {
        return Err(BlacklistError::CrateAlreadyOnBlacklist(name.into()).into());
    }

    conn.execute(
        "INSERT INTO blacklisted_crates (crate_name) VALUES ($1);",
        &[&name],
    )?;

    Ok(())
}

/// Removes a crate from the blacklist.
pub fn remove_crate(conn: &mut Client, name: &str) -> Result<()> {
    if !is_blacklisted(conn, name)? {
        return Err(BlacklistError::CrateNotOnBlacklist(name.into()).into());
    }

    conn.execute(
        "DELETE FROM blacklisted_crates WHERE crate_name = $1;",
        &[&name],
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_blacklist() {
        crate::test::wrapper(|env| {
            let db = env.db();

            // crates are added out of order to verify sorting
            add_crate(&mut db.conn(), "crate A")?;
            add_crate(&mut db.conn(), "crate C")?;
            add_crate(&mut db.conn(), "crate B")?;

            assert!(list_crates(&mut db.conn())? == vec!["crate A", "crate B", "crate C"]);
            Ok(())
        });
    }

    #[test]
    fn test_add_to_and_remove_from_blacklist() {
        crate::test::wrapper(|env| {
            let db = env.db();

            assert!(!is_blacklisted(&mut db.conn(), "crate foo")?);
            add_crate(&mut db.conn(), "crate foo")?;
            assert!(is_blacklisted(&mut db.conn(), "crate foo")?);
            remove_crate(&mut db.conn(), "crate foo")?;
            assert!(!is_blacklisted(&mut db.conn(), "crate foo")?);
            Ok(())
        });
    }

    #[test]
    fn test_add_twice_to_blacklist() {
        crate::test::wrapper(|env| {
            let db = env.db();

            add_crate(&mut db.conn(), "crate foo")?;
            assert!(add_crate(&mut db.conn(), "crate foo").is_err());
            add_crate(&mut db.conn(), "crate bar")?;

            Ok(())
        });
    }

    #[test]
    fn test_remove_non_existing_crate() {
        crate::test::wrapper(|env| {
            let db = env.db();

            assert!(remove_crate(&mut db.conn(), "crate foo").is_err());

            Ok(())
        });
    }
}
