use crate::error::Result;
use futures_util::stream::TryStreamExt;
use std::time::Duration;

#[derive(Default, Debug, Clone, Copy, Eq, PartialEq)]
pub struct Overrides {
    pub memory: Option<usize>,
    pub targets: Option<usize>,
    pub timeout: Option<Duration>,
}

macro_rules! row_to_overrides {
    ($row:expr) => {{
        Overrides {
            memory: $row.max_memory_bytes.map(|i| i as usize),
            targets: $row.max_targets.map(|i| i as usize),
            timeout: $row.timeout_seconds.map(|i| Duration::from_secs(i as u64)),
        }
    }};
}

impl Overrides {
    pub async fn all(conn: &mut sqlx::PgConnection) -> Result<Vec<(String, Self)>> {
        Ok(sqlx::query!("SELECT * FROM sandbox_overrides")
            .fetch(conn)
            .map_ok(|row| (row.crate_name, row_to_overrides!(row)))
            .try_collect()
            .await?)
    }

    pub async fn for_crate(conn: &mut sqlx::PgConnection, krate: &str) -> Result<Option<Self>> {
        Ok(sqlx::query!(
            "SELECT * FROM sandbox_overrides WHERE crate_name = $1",
            krate
        )
        .fetch_optional(conn)
        .await?
        .map(|row| row_to_overrides!(row)))
    }

    pub async fn save(conn: &mut sqlx::PgConnection, krate: &str, overrides: Self) -> Result<()> {
        if overrides.timeout.is_some() && overrides.targets.is_none() {
            tracing::warn!(
                "setting `Overrides::timeout` implies a default `Overrides::targets = 1`, prefer setting this explicitly"
            );
        }

        if sqlx::query_scalar!("SELECT id FROM crates WHERE crates.name = $1", krate)
            .fetch_optional(&mut *conn)
            .await?
            .is_none()
        {
            tracing::warn!("setting overrides for unknown crate `{krate}`");
        }

        sqlx::query!(
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
            krate,
            overrides.memory.map(|i| i as i64),
            overrides.targets.map(|i| i as i32),
            overrides.timeout.map(|d| d.as_secs() as i32),
        )
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    pub async fn remove(conn: &mut sqlx::PgConnection, krate: &str) -> Result<()> {
        sqlx::query!("DELETE FROM sandbox_overrides WHERE crate_name = $1", krate)
            .execute(conn)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::{db::Overrides, test::*};
    use std::time::Duration;

    #[test]
    fn retrieve_overrides() {
        async_wrapper(|env| async move {
            let db = env.async_db();
            let mut conn = db.async_conn().await;

            let krate = "hexponent";

            // no overrides
            let actual = Overrides::for_crate(&mut conn, krate).await?;
            assert_eq!(actual, None);

            // add partial overrides
            let expected = Overrides {
                targets: Some(1),
                ..Overrides::default()
            };
            Overrides::save(&mut conn, krate, expected).await?;
            let actual = Overrides::for_crate(&mut conn, krate).await?;
            assert_eq!(actual, Some(expected));

            // overwrite with full overrides
            let expected = Overrides {
                memory: Some(100_000),
                targets: Some(1),
                timeout: Some(Duration::from_secs(300)),
            };
            Overrides::save(&mut conn, krate, expected).await?;
            let actual = Overrides::for_crate(&mut conn, krate).await?;
            assert_eq!(actual, Some(expected));

            // overwrite with partial overrides
            let expected = Overrides {
                memory: Some(1),
                ..Overrides::default()
            };
            Overrides::save(&mut conn, krate, expected).await?;
            let actual = Overrides::for_crate(&mut conn, krate).await?;
            assert_eq!(actual, Some(expected));

            // remove overrides
            Overrides::remove(&mut conn, krate).await?;
            let actual = Overrides::for_crate(&mut conn, krate).await?;
            assert_eq!(actual, None);

            Ok(())
        })
    }
}
