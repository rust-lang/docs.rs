use super::{Blob, StorageTransaction};
use crate::db::Pool;
use chrono::{DateTime, NaiveDateTime, Utc};
use failure::{Error, Fail};
use postgres::transaction::Transaction;

#[derive(Debug, Fail)]
#[fail(display = "the path is not present in the database")]
struct PathNotFoundError;

pub(crate) struct DatabaseBackend {
    pool: Pool,
}

impl DatabaseBackend {
    pub(crate) fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub(super) fn get(&self, path: &str, max_size: usize) -> Result<Blob, Error> {
        use std::convert::TryInto;

        // The maximum size for a BYTEA (the type used for `content`) is 1GB, so this cast is safe:
        // https://www.postgresql.org/message-id/162867790712200946i7ba8eb92v908ac595c0c35aee%40mail.gmail.com
        let max_size = max_size.min(std::i32::MAX as usize) as i32;

        // The size limit is checked at the database level, to avoid receiving data altogether if
        // the limit is exceeded.
        let rows = self.pool.get()?.query(
            "SELECT
                 path, mime, date_updated, compression,
                 (CASE WHEN LENGTH(content) <= $2 THEN content ELSE NULL END) AS content,
                 (LENGTH(content) > $2) AS is_too_big
             FROM files
             WHERE path = $1;",
            &[&path, &(max_size)],
        )?;

        if rows.is_empty() {
            Err(PathNotFoundError.into())
        } else {
            let row = rows.get(0);

            if row.get("is_too_big") {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    crate::error::SizeLimitReached,
                )
                .into());
            }

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

    pub(super) fn start_connection(&self) -> Result<DatabaseConnection, Error> {
        Ok(DatabaseConnection {
            conn: self.pool.get()?,
        })
    }
}

pub(super) struct DatabaseConnection {
    conn: crate::db::PoolConnection,
}

impl DatabaseConnection {
    pub(super) fn start_storage_transaction(
        &self,
    ) -> Result<DatabaseStorageTransaction<'_>, Error> {
        Ok(DatabaseStorageTransaction {
            transaction: Some(self.conn.transaction()?),
        })
    }
}

pub(super) struct DatabaseStorageTransaction<'a> {
    transaction: Option<Transaction<'a>>,
}

impl<'a> StorageTransaction for DatabaseStorageTransaction<'a> {
    fn store_batch(&mut self, batch: &[Blob]) -> Result<(), Error> {
        let transaction = self
            .transaction
            .as_ref()
            .expect("called complete() before store_batch()");

        for blob in batch {
            let compression = blob.compression.map(|alg| alg as i32);
            transaction.query(
                "INSERT INTO files (path, mime, content, compression)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (path) DO UPDATE
                    SET mime = EXCLUDED.mime, content = EXCLUDED.content, compression = EXCLUDED.compression",
                &[&blob.path, &blob.mime, &blob.content, &compression],
            )?;
        }
        Ok(())
    }

    fn complete(&mut self) -> Result<(), Error> {
        self.transaction
            .take()
            .expect("called complete() multiple times")
            .commit()?;
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
            let db = env.db();
            let conn = db.conn();
            let backend = DatabaseBackend::new(db.pool());
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
            let db = env.db();
            let backend = DatabaseBackend::new(db.pool());

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

            let conn = backend.start_connection()?;
            let mut transaction = conn.start_storage_transaction()?;
            transaction.store_batch(std::slice::from_ref(&small_blob))?;
            transaction.store_batch(std::slice::from_ref(&big_blob))?;
            transaction.complete()?;

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
