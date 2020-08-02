use super::{Blob, StorageTransaction};
use crate::db::Pool;
use chrono::{DateTime, NaiveDateTime, Utc};
use failure::Error;
use postgres::Transaction;

pub(crate) struct DatabaseBackend {
    pool: Pool,
}

impl DatabaseBackend {
    pub(crate) fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub(super) fn exists(&self, path: &str) -> Result<bool, Error> {
        let query = "SELECT COUNT(*) > 0 FROM files WHERE path = $1";
        let mut conn = self.pool.get()?;
        Ok(conn.query(query, &[&path])?[0].get(0))
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
            Err(super::PathNotFoundError.into())
        } else {
            let row = &rows[0];

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
        &mut self,
    ) -> Result<DatabaseStorageTransaction<'_>, Error> {
        Ok(DatabaseStorageTransaction {
            transaction: self.conn.transaction()?,
        })
    }
}

pub(super) struct DatabaseStorageTransaction<'a> {
    transaction: Transaction<'a>,
}

impl<'a> StorageTransaction for DatabaseStorageTransaction<'a> {
    fn store_batch(&mut self, batch: Vec<Blob>) -> Result<(), Error> {
        for blob in batch {
            let compression = blob.compression.map(|alg| alg as i32);
            self.transaction.query(
                "INSERT INTO files (path, mime, content, compression)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (path) DO UPDATE
                    SET mime = EXCLUDED.mime, content = EXCLUDED.content, compression = EXCLUDED.compression",
                &[&blob.path, &blob.mime, &blob.content, &compression],
            )?;
        }
        Ok(())
    }

    fn delete_prefix(&mut self, prefix: &str) -> Result<(), Error> {
        self.transaction.execute(
            "DELETE FROM files WHERE path LIKE $1;",
            &[&format!("{}%", prefix.replace('%', "\\%"))],
        )?;
        Ok(())
    }

    fn complete(self: Box<Self>) -> Result<(), Error> {
        self.transaction.commit()?;
        Ok(())
    }
}

// The tests for this module are in src/storage/mod.rs, as part of the backend tests. Please add
// any test checking the public interface there.
