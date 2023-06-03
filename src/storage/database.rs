use super::{Blob, FileRange, StorageTransaction};
use crate::db::Pool;
use crate::error::Result;
use crate::InstanceMetrics;
use postgres::Transaction;
use std::{convert::TryFrom, sync::Arc};

pub(crate) struct DatabaseBackend {
    pool: Pool,
    metrics: Arc<InstanceMetrics>,
}

impl DatabaseBackend {
    pub(crate) fn new(pool: Pool, metrics: Arc<InstanceMetrics>) -> Self {
        Self { pool, metrics }
    }

    pub(super) fn exists(&self, path: &str) -> Result<bool> {
        let query = "SELECT COUNT(*) > 0 FROM files WHERE path = $1";
        let mut conn = self.pool.get()?;
        Ok(conn.query(query, &[&path])?[0].get(0))
    }

    pub(super) fn get_public_access(&self, path: &str) -> Result<bool> {
        match self.pool.get()?.query_opt(
            "SELECT public 
             FROM files 
             WHERE path = $1",
            &[&path],
        )? {
            Some(row) => Ok(row.get(0)),
            None => Err(super::PathNotFoundError.into()),
        }
    }

    pub(super) fn set_public_access(&self, path: &str, public: bool) -> Result<()> {
        if self.pool.get()?.execute(
            "UPDATE files 
             SET public = $2 
             WHERE path = $1",
            &[&path, &public],
        )? == 1
        {
            Ok(())
        } else {
            Err(super::PathNotFoundError.into())
        }
    }

    pub(super) fn get(
        &self,
        path: &str,
        max_size: usize,
        range: Option<FileRange>,
    ) -> Result<Blob> {
        // The maximum size for a BYTEA (the type used for `content`) is 1GB, so this cast is safe:
        // https://www.postgresql.org/message-id/162867790712200946i7ba8eb92v908ac595c0c35aee%40mail.gmail.com
        let max_size = max_size.min(std::i32::MAX as usize) as i32;

        let rows = if let Some(r) = range {
            // when we only want to get a range we can validate already if the range is small enough
            if (r.end() - r.start() + 1) > max_size as u64 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    crate::error::SizeLimitReached,
                )
                .into());
            }
            let range_start = i32::try_from(*r.start())?;

            self.pool.get()?.query(
                "SELECT
                     path, mime, date_updated, compression,
                     substring(content from $2 for $3) as content,
                     FALSE as is_too_big
                 FROM files
                 WHERE path = $1;",
                &[
                    &path,
                    &(range_start + 1), // postgres substring is 1-indexed
                    &((r.end() - r.start() + 1) as i32),
                ],
            )?
        } else {
            // The size limit is checked at the database level, to avoid receiving data altogether if
            // the limit is exceeded.
            self.pool.get()?.query(
                "SELECT
                     path, mime, date_updated, compression,
                     (CASE WHEN LENGTH(content) <= $2 THEN content ELSE NULL END) AS content,
                     (LENGTH(content) > $2) AS is_too_big
                 FROM files
                 WHERE path = $1;",
                &[&path, &(max_size)],
            )?
        };

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
                date_updated: row.get("date_updated"),
                content: row.get("content"),
                compression,
            })
        }
    }

    pub(super) fn start_connection(&self) -> Result<DatabaseClient> {
        Ok(DatabaseClient {
            conn: self.pool.get()?,
            metrics: self.metrics.clone(),
        })
    }
}

pub(super) struct DatabaseClient {
    conn: crate::db::PoolClient,
    metrics: Arc<InstanceMetrics>,
}

impl DatabaseClient {
    pub(super) fn start_storage_transaction(&mut self) -> Result<DatabaseStorageTransaction<'_>> {
        Ok(DatabaseStorageTransaction {
            transaction: self.conn.transaction()?,
            metrics: &self.metrics,
        })
    }
}

pub(super) struct DatabaseStorageTransaction<'a> {
    transaction: Transaction<'a>,
    metrics: &'a InstanceMetrics,
}

impl<'a> StorageTransaction for DatabaseStorageTransaction<'a> {
    fn store_batch(&mut self, batch: Vec<Blob>) -> Result<()> {
        for blob in batch {
            let compression = blob.compression.map(|alg| alg as i32);
            self.transaction.query(
                "INSERT INTO files (path, mime, content, compression)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (path) DO UPDATE
                    SET mime = EXCLUDED.mime, content = EXCLUDED.content, compression = EXCLUDED.compression",
                &[&blob.path, &blob.mime, &blob.content, &compression],
            )?;
            self.metrics.uploaded_files_total.inc();
        }
        Ok(())
    }

    fn delete_prefix(&mut self, prefix: &str) -> Result<()> {
        self.transaction.execute(
            "DELETE FROM files WHERE path LIKE $1;",
            &[&format!("{}%", prefix.replace('%', "\\%"))],
        )?;
        Ok(())
    }

    fn complete(self: Box<Self>) -> Result<()> {
        self.transaction.commit()?;
        Ok(())
    }
}

// The tests for this module are in src/storage/mod.rs, as part of the backend tests. Please add
// any test checking the public interface there.
