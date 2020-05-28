use super::Blob;
use chrono::{DateTime, NaiveDateTime, Utc};
use failure::{Error, Fail};
use postgres::{transaction::Transaction, Connection};

#[derive(Debug, Fail)]
#[fail(display = "the path is not present in the database")]
struct PathNotFoundError;

pub(crate) struct DatabaseBackend<'a> {
    conn: &'a Connection,
}

impl<'a> DatabaseBackend<'a> {
    pub(crate) fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub(super) fn get(&self, path: &str) -> Result<Blob, Error> {
        let rows = self.conn.query(
            "SELECT path, mime, date_updated, content, compression FROM files WHERE path = $1;",
            &[&path],
        )?;

        if rows.is_empty() {
            Err(PathNotFoundError.into())
        } else {
            let row = rows.get(0);
            let compression = row.get::<_, Option<String>>("compression").map(|alg| {
                alg.parse()
                    .expect("invalid or unknown compression algorithm")
            });
            Ok(Blob {
                path: row.get("path"),
                mime: row.get("mime"),
                date_updated: DateTime::from_utc(row.get::<_, NaiveDateTime>("date_updated"), Utc),
                content: row.get("content"),
                compression,
            })
        }
    }

    pub(super) fn store_batch(&self, batch: &[Blob], trans: &Transaction) -> Result<(), Error> {
        for blob in batch {
            let compression = blob.compression.as_ref().map(|alg| alg.to_string());
            trans.query(
                "INSERT INTO files (path, mime, content, compression)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (path) DO UPDATE
                    SET mime = EXCLUDED.mime, content = EXCLUDED.content, compression = EXCLUDED.compression",
                &[&blob.path, &blob.mime, &blob.content, &compression],
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{SubsecRound, Utc};

    #[test]
    fn test_path_get() {
        crate::test::wrapper(|env| {
            let conn = env.db().conn();
            let backend = DatabaseBackend::new(&conn);
            let now = Utc::now();

            // Add a test file to the database
            conn.execute(
                "INSERT INTO files (path, mime, date_updated, content) VALUES ($1, $2, $3, $4);",
                &[
                    &"dir/foo.txt",
                    &"text/plain",
                    &now.naive_utc(),
                    &"Hello world!".as_bytes(),
                ],
            )?;

            // Test that the proper file was returned
            assert_eq!(
                Blob {
                    path: "dir/foo.txt".into(),
                    mime: "text/plain".into(),
                    date_updated: now.trunc_subsecs(6),
                    content: "Hello world!".bytes().collect(),
                    compression: None,
                },
                backend.get("dir/foo.txt")?
            );

            // Test that other files are not returned
            assert!(backend
                .get("dir/bar.txt")
                .unwrap_err()
                .downcast_ref::<PathNotFoundError>()
                .is_some());
            assert!(backend
                .get("foo.txt")
                .unwrap_err()
                .downcast_ref::<PathNotFoundError>()
                .is_some());

            Ok(())
        });
    }
}
