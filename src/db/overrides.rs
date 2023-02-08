use crate::error::Result;
use postgres::Client;
use std::time::Duration;

#[derive(Default, Debug, Clone, Copy, Eq, PartialEq)]
pub struct Overrides {
    pub memory: Option<usize>,
    pub targets: Option<usize>,
    pub timeout: Option<Duration>,
}

impl Overrides {
    pub fn all(conn: &mut Client) -> Result<Vec<(String, Self)>> {
        Ok(conn
            .query("SELECT * FROM sandbox_overrides", &[])?
            .into_iter()
            .map(|row| (row.get("crate_name"), Self::from_row(row)))
            .collect())
    }

    pub fn for_crate(conn: &mut Client, krate: &str) -> Result<Option<Self>> {
        Ok(conn
            .query_opt(
                "SELECT * FROM sandbox_overrides WHERE crate_name = $1",
                &[&krate],
            )?
            .map(Self::from_row))
    }

    fn from_row(row: postgres::Row) -> Self {
        Self {
            memory: row
                .get::<_, Option<i64>>("max_memory_bytes")
                .map(|i| i as usize),
            targets: row.get::<_, Option<i32>>("max_targets").map(|i| i as usize),
            timeout: row
                .get::<_, Option<i32>>("timeout_seconds")
                .map(|i| Duration::from_secs(i as u64)),
        }
    }

    pub fn save(conn: &mut Client, krate: &str, overrides: Self) -> Result<()> {
        if overrides.timeout.is_some() && overrides.targets.is_none() {
            tracing::warn!("setting `Overrides::timeout` implies a default `Overrides::targets = 1`, prefer setting this explicitly");
        }
        conn.execute(
            "
                INSERT INTO sandbox_overrides (
                    crate_name, max_memory_bytes, max_targets, timeout_seconds
                )
                VALUES ($1, $2, $3, $4)
                ON CONFLICT (crate_name) DO UPDATE
                    SET
                        max_memory_bytes = $2,
                        max_targets = $3,
                        timeout_seconds = $4
                ",
            &[
                &krate,
                &overrides.memory.map(|i| i as i64),
                &overrides.targets.map(|i| i as i32),
                &overrides.timeout.map(|d| d.as_secs() as i32),
            ],
        )?;
        Ok(())
    }

    pub fn remove(conn: &mut Client, krate: &str) -> Result<()> {
        conn.execute(
            "DELETE FROM sandbox_overrides WHERE crate_name = $1",
            &[&krate],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::{db::Overrides, test::*};
    use std::time::Duration;

    #[test]
    fn retrieve_overrides() {
        wrapper(|env| {
            let db = env.db();

            let krate = "hexponent";

            // no overrides
            let actual = Overrides::for_crate(&mut db.conn(), krate)?;
            assert_eq!(actual, None);

            // add partial overrides
            let expected = Overrides {
                targets: Some(1),
                ..Overrides::default()
            };
            Overrides::save(&mut db.conn(), krate, expected)?;
            let actual = Overrides::for_crate(&mut db.conn(), krate)?;
            assert_eq!(actual, Some(expected));

            // overwrite with full overrides
            let expected = Overrides {
                memory: Some(100_000),
                targets: Some(1),
                timeout: Some(Duration::from_secs(300)),
            };
            Overrides::save(&mut db.conn(), krate, expected)?;
            let actual = Overrides::for_crate(&mut db.conn(), krate)?;
            assert_eq!(actual, Some(expected));

            // overwrite with partial overrides
            let expected = Overrides {
                memory: Some(1),
                ..Overrides::default()
            };
            Overrides::save(&mut db.conn(), krate, expected)?;
            let actual = Overrides::for_crate(&mut db.conn(), krate)?;
            assert_eq!(actual, Some(expected));

            // remove overrides
            Overrides::remove(&mut db.conn(), krate)?;
            let actual = Overrides::for_crate(&mut db.conn(), krate)?;
            assert_eq!(actual, None);

            Ok(())
        });
    }
}
