use super::{BlobUpload, FileRange, StorageMetrics, StreamingBlob};
use crate::{InstanceMetrics, db::Pool, error::Result};
use chrono::{DateTime, Utc};
use futures_util::stream::{Stream, TryStreamExt};
use sqlx::Acquire;
use std::{io, sync::Arc};

pub(crate) struct DatabaseBackend {
    pool: Pool,
    metrics: Arc<InstanceMetrics>,
    otel_metrics: StorageMetrics,
}

impl DatabaseBackend {
    pub(crate) fn new(
        pool: Pool,
        metrics: Arc<InstanceMetrics>,
        otel_metrics: StorageMetrics,
    ) -> Self {
        Self {
            pool,
            metrics,
            otel_metrics,
        }
    }

    pub(super) async fn exists(&self, path: &str) -> Result<bool> {
        Ok(sqlx::query_scalar!(
            r#"SELECT COUNT(*) > 0 as "has_count!" FROM files WHERE path = $1"#,
            path
        )
        .fetch_one(&self.pool)
        .await?)
    }

    pub(super) async fn get_public_access(&self, path: &str) -> Result<bool> {
        match sqlx::query_scalar!(
            "SELECT public
             FROM files
             WHERE path = $1",
            path
        )
        .fetch_optional(&self.pool)
        .await?
        {
            Some(public) => Ok(public),
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

    pub(super) async fn get_stream(
        &self,
        path: &str,
        range: Option<FileRange>,
    ) -> Result<StreamingBlob> {
        struct Result {
            path: String,
            mime: String,
            date_updated: DateTime<Utc>,
            compression: Option<i32>,
            content: Option<Vec<u8>>,
        }

        let result = if let Some(r) = range {
            let range_start = i32::try_from(*r.start())?;

            sqlx::query_as!(
                Result,
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
            .ok_or(super::PathNotFoundError)?
        } else {
            // The size limit is checked at the database level, to avoid receiving data altogether if
            // the limit is exceeded.
            sqlx::query_as!(
                Result,
                r#"SELECT
                     path,
                     mime,
                     date_updated,
                     compression,
                     content
                 FROM files
                 WHERE path = $1;"#,
                path,
            )
            .fetch_optional(&self.pool)
            .await?
            .ok_or(super::PathNotFoundError)?
        };

        let compression = result.compression.map(|i| {
            i.try_into()
                .expect("invalid compression algorithm stored in database")
        });
        let content = result.content.unwrap_or_default();
        let content_len = content.len();
        Ok(StreamingBlob {
            path: result.path,
            mime: result
                .mime
                .parse()
                .unwrap_or(mime::APPLICATION_OCTET_STREAM),
            date_updated: result.date_updated,
            content: Box::new(io::Cursor::new(content)),
            content_length: content_len,
            compression,
        })
    }

    pub(super) async fn store_batch(&self, batch: Vec<BlobUpload>) -> Result<()> {
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
                &blob.mime.to_string(),
                &blob.content,
                compression,
            )
            .execute(&mut *trans).await?;
            self.metrics.uploaded_files_total.inc();
            self.otel_metrics.uploaded_files.add(1, &[]);
        }
        trans.commit().await?;
        Ok(())
    }

    pub(super) async fn list_prefix<'a>(
        &'a self,
        prefix: &'a str,
    ) -> impl Stream<Item = Result<String>> + 'a {
        sqlx::query!(
            "SELECT path
             FROM files
             WHERE path LIKE $1
             ORDER BY path;",
            format!("{}%", prefix.replace('%', "\\%"))
        )
        .fetch(&self.pool)
        .map_err(Into::into)
        .map_ok(|row| row.path)
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
