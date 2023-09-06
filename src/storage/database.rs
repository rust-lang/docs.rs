use sqlx::Acquire;

use super::{Blob, FileRange};
use crate::db::Pool;
use crate::error::Result;
use crate::InstanceMetrics;
use std::{convert::TryFrom, sync::Arc};

pub(crate) struct DatabaseBackend {
    pool: Pool,
    metrics: Arc<InstanceMetrics>,
}

impl DatabaseBackend {
    pub(crate) fn new(pool: Pool, metrics: Arc<InstanceMetrics>) -> Self {
        Self { pool, metrics }
    }

    pub(super) async fn exists(&self, path: &str) -> Result<bool> {
        Ok(sqlx::query!(
            r#"SELECT COUNT(*) > 0 as "has_count!" FROM files WHERE path = $1"#,
            path
        )
        .fetch_one(&self.pool)
        .await?
        .has_count)
    }

    pub(super) async fn get_public_access(&self, path: &str) -> Result<bool> {
        match sqlx::query!(
            "SELECT public 
             FROM files 
             WHERE path = $1",
            path
        )
        .fetch_optional(&self.pool)
        .await?
        {
            Some(row) => Ok(row.public),
            None => Err(super::PathNotFoundError.into()),
        }
    }

    pub(super) async fn set_public_access(&self, path: &str, public: bool) -> Result<()> {
        if sqlx::query!(
            "UPDATE files 
             SET public = $2 
             WHERE path = $1",
            path,
            public,
        )
        .execute(&self.pool)
        .await?
        .rows_affected()
            == 1
        {
            Ok(())
        } else {
            Err(super::PathNotFoundError.into())
        }
    }

    pub(super) async fn get(
        &self,
        path: &str,
        max_size: usize,
        range: Option<FileRange>,
    ) -> Result<Blob> {
        // The maximum size for a BYTEA (the type used for `content`) is 1GB, so this cast is safe:
        // https://www.postgresql.org/message-id/162867790712200946i7ba8eb92v908ac595c0c35aee%40mail.gmail.com
        let max_size = max_size.min(std::i32::MAX as usize) as i32;

        let (path, mime, date_updated, compression, content, is_too_big) = if let Some(r) = range {
            // when we only want to get a range we can validate already if the range is small enough
            if (r.end() - r.start() + 1) > max_size as u64 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    crate::error::SizeLimitReached,
                )
                .into());
            }
            let range_start = i32::try_from(*r.start())?;

            sqlx::query!(
                r#"SELECT
                     path, mime, date_updated, compression,
                     substring(content from $2 for $3) as content
                 FROM files
                 WHERE path = $1;"#,
                path,
                range_start + 1, // postgres substring is 1-indexed
                (r.end() - r.start() + 1) as i32
            )
            .fetch_optional(&self.pool)
            .await?
            .ok_or(super::PathNotFoundError)
            .map(|row| {
                (
                    row.path,
                    row.mime,
                    row.date_updated,
                    row.compression,
                    row.content,
                    false,
                )
            })?
        } else {
            // The size limit is checked at the database level, to avoid receiving data altogether if
            // the limit is exceeded.
            sqlx::query!(
                r#"SELECT
                     path, mime, date_updated, compression,
                     (CASE WHEN LENGTH(content) <= $2 THEN content ELSE NULL END) AS content,
                     (LENGTH(content) > $2) AS "is_too_big!"
                 FROM files
                 WHERE path = $1;"#,
                path,
                max_size,
            )
            .fetch_optional(&self.pool)
            .await?
            .ok_or(super::PathNotFoundError)
            .map(|row| {
                (
                    row.path,
                    row.mime,
                    row.date_updated,
                    row.compression,
                    row.content,
                    row.is_too_big,
                )
            })?
        };

        if is_too_big {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                crate::error::SizeLimitReached,
            )
            .into());
        }

        let compression = compression.map(|i| {
            i.try_into()
                .expect("invalid compression algorithm stored in database")
        });
        Ok(Blob {
            path,
            mime,
            date_updated,
            content: content.unwrap_or_default(),
            compression,
        })
    }

    pub(super) async fn store_batch(&self, batch: Vec<Blob>) -> Result<()> {
        let mut conn = self.pool.get_async().await?;
        let mut trans = conn.begin().await?;
        for blob in batch {
            let compression = blob.compression.map(|alg| alg as i32);
            sqlx::query!(
                "INSERT INTO files (path, mime, content, compression)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (path) DO UPDATE
                    SET mime = EXCLUDED.mime, content = EXCLUDED.content, compression = EXCLUDED.compression",
                &blob.path,
                &blob.mime,
                &blob.content,
                compression,
            )
            .execute(&mut *trans).await?;
            self.metrics.uploaded_files_total.inc();
        }
        trans.commit().await?;
        Ok(())
    }

    pub(crate) async fn delete_prefix(&self, prefix: &str) -> Result<()> {
        sqlx::query!(
            "DELETE FROM files WHERE path LIKE $1;",
            format!("{}%", prefix.replace('%', "\\%"))
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

// The tests for this module are in src/storage/mod.rs, as part of the backend tests. Please add
// any test checking the public interface there.
