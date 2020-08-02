use failure::{Error, Fail};
use postgres::Client as Connection;

#[derive(Debug, Fail)]
enum BlacklistError {
    #[fail(display = "crate {} is already on the blacklist", _0)]
    CrateAlreadyOnBlacklist(String),

    #[fail(display = "crate {} is not on the blacklist", _0)]
    CrateNotOnBlacklist(String),
}

/// Returns whether the given name is blacklisted.
pub fn is_blacklisted(conn: &mut Connection, name: &str) -> Result<bool, Error> {
    let rows = conn.query(
        "SELECT COUNT(*) FROM blacklisted_crates WHERE crate_name = $1;",
        &[&name],
    )?;
    let count: i64 = rows[0].get(0);

    Ok(count != 0)
}

/// Returns the crate names on the blacklist, sorted ascending.
pub fn list_crates(conn: &mut Connection) -> Result<Vec<String>, Error> {
    let rows = conn.query(
        "SELECT crate_name FROM blacklisted_crates ORDER BY crate_name asc;",
        &[],
    )?;

    Ok(rows.into_iter().map(|row| row.get(0)).collect())
}

/// Adds a crate to the blacklist.
pub fn add_crate(conn: &mut Connection, name: &str) -> Result<(), Error> {
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
pub fn remove_crate(conn: &mut Connection, name: &str) -> Result<(), Error> {
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
            add_crate(&db.conn(), "crate A")?;
            add_crate(&db.conn(), "crate C")?;
            add_crate(&db.conn(), "crate B")?;

            assert!(list_crates(&db.conn())? == vec!["crate A", "crate B", "crate C"]);
            Ok(())
        });
    }

    #[test]
    fn test_add_to_and_remove_from_blacklist() {
        crate::test::wrapper(|env| {
            let db = env.db();

            assert!(!is_blacklisted(&db.conn(), "crate foo")?);
            add_crate(&db.conn(), "crate foo")?;
            assert!(is_blacklisted(&db.conn(), "crate foo")?);
            remove_crate(&db.conn(), "crate foo")?;
            assert!(!is_blacklisted(&db.conn(), "crate foo")?);
            Ok(())
        });
    }

    #[test]
    fn test_add_twice_to_blacklist() {
        crate::test::wrapper(|env| {
            let db = env.db();

            add_crate(&db.conn(), "crate foo")?;
            assert!(add_crate(&db.conn(), "crate foo").is_err());
            add_crate(&db.conn(), "crate bar")?;

            Ok(())
        });
    }

    #[test]
    fn test_remove_non_existing_crate() {
        crate::test::wrapper(|env| {
            let db = env.db();

            assert!(remove_crate(&db.conn(), "crate foo").is_err());

            Ok(())
        });
    }
}
