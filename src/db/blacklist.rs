use failure::Error;
use postgres::Connection;

#[derive(Debug, Fail)]
enum BlacklistError {
    #[fail(display = "crate {} is already on the blacklist", _0)]
    CrateAlreadyOnBlacklist(String),

    #[fail(display = "crate {} is not on the blacklist", _0)]
    CrateNotOnBlacklist(String),
}

pub fn is_blacklisted(conn: &Connection, name: &str) -> Result<bool, Error> {
    let rows = conn.query(
        "SELECT COUNT(*) FROM blacklisted_crates WHERE crate_name = $1;",
        &[&name],
    )?;
    let count: i64 = rows.get(0).get(0);

    Ok(count != 0)
}

pub fn add_crate(conn: &Connection, name: &str) -> Result<(), Error> {
    if is_blacklisted(conn, name)? {
        return Err(BlacklistError::CrateAlreadyOnBlacklist(name.into()).into());
    }

    conn.execute(
        "INSERT INTO blacklisted_crates (crate_name) VALUES ($1);",
        &[&name],
    )?;

    Ok(())
}

pub fn remove_crate(conn: &Connection, name: &str) -> Result<(), Error> {
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
    fn test_add_to_and_remove_from_blacklist() {
        crate::test::with_database(|db| {
            assert!(is_blacklisted(db.conn(), "crate foo")? == false);
            add_crate(db.conn(), "crate foo")?;
            assert!(is_blacklisted(db.conn(), "crate foo")? == true);
            remove_crate(db.conn(), "crate foo")?;
            assert!(is_blacklisted(db.conn(), "crate foo")? == false);
            Ok(())
        });
    }

    #[test]
    fn test_add_twice_to_blacklist() {
        crate::test::with_database(|db| {
            add_crate(db.conn(), "crate foo")?;
            assert!(add_crate(db.conn(), "crate foo").is_err());
            add_crate(db.conn(), "crate bar")?;

            Ok(())
        });
    }

    #[test]
    fn test_remove_non_existing_crate() {
        crate::test::with_database(|db| {
            assert!(remove_crate(db.conn(), "crate foo").is_err());

            Ok(())
        });
    }
}
