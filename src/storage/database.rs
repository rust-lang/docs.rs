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

    pub(super) fn get(&self, path: &str, max_size: usize) -> Result<Blob, Error> {
        use std::convert::TryInto;

        // The maximum size for a BYTEA (the type used for `content`) is 1GB, so this cast is safe:
        // https://www.postgresql.org/message-id/162867790712200946i7ba8eb92v908ac595c0c35aee%40mail.gmail.com
        let max_size = max_size.min(std::i32::MAX as usize) as i32;

        // The size limit is checked at the database level, to avoid receiving data altogether if
        // the limit is exceeded.
        let rows = self.conn.query(
            "SELECT path, mime, date_updated, content, compression
             FROM files
             WHERE path = $1 AND LENGTH(content) <= $2;",
            &[&path, &(max_size)],
        )?;

        if rows.is_empty() {
            // This second query distinguishes between a path not found error and a size limit
            // reached error, as the above query returns no result in either cases.
            if self
                .conn
                .query("SELECT 0 FROM files WHERE path = $1;", &[&path])?
                .is_empty()
            {
                Err(PathNotFoundError.into())
            } else {
                Err(
                    std::io::Error::new(std::io::ErrorKind::Other, crate::error::SizeLimitReached)
                        .into(),
                )
            }
        } else {
            let row = rows.get(0);
            let compression = row.get::<_, Option<i32>>("compression").map(|i| {
                i.try_into()
                    .expect("invalid compression algorithm stored in database")
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
            let compression = blob.compression.map(|alg| alg as i32);
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
                backend.get("dir/foo.txt", std::usize::MAX)?
            );

            // Test that other files are not returned
            assert!(backend
                .get("dir/bar.txt", std::usize::MAX)
                .unwrap_err()
                .downcast_ref::<PathNotFoundError>()
                .is_some());
            assert!(backend
                .get("foo.txt", std::usize::MAX)
                .unwrap_err()
                .downcast_ref::<PathNotFoundError>()
                .is_some());

            Ok(())
        });
    }

    #[test]
    fn test_get_too_big() {
        const MAX_SIZE: usize = 1024;

        crate::test::wrapper(|env| {
            let conn = env.db().conn();
            let backend = DatabaseBackend::new(&conn);

            let small_blob = Blob {
                path: "small-blob.bin".into(),
                mime: "text/plain".into(),
                date_updated: Utc::now(),
                content: vec![0; MAX_SIZE],
                compression: None,
            };
            let big_blob = Blob {
                path: "big-blob.bin".into(),
                mime: "text/plain".into(),
                date_updated: Utc::now(),
                content: vec![0; MAX_SIZE * 2],
                compression: None,
            };

            let transaction = conn.transaction()?;
            backend
                .store_batch(std::slice::from_ref(&small_blob), &transaction)
                .unwrap();
            backend
                .store_batch(std::slice::from_ref(&big_blob), &transaction)
                .unwrap();
            transaction.commit()?;

            let blob = backend.get("small-blob.bin", MAX_SIZE).unwrap();
            assert_eq!(blob.content.len(), small_blob.content.len());

            assert!(backend
                .get("big-blob.bin", MAX_SIZE)
                .unwrap_err()
                .downcast_ref::<std::io::Error>()
                .and_then(|io| io.get_ref())
                .and_then(|err| err.downcast_ref::<crate::error::SizeLimitReached>())
                .is_some());

            Ok(())
        });
    }
}
