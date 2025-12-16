use super::{BlobUpload, FileRange, StorageMetrics, StreamingBlob};
use crate::{db::Pool, error::Result};
use chrono::{DateTime, Utc};
use docs_rs_headers::compute_etag;
use futures_util::stream::{Stream, TryStreamExt};
use sqlx::Acquire;
use std::io;

pub(crate) struct DatabaseBackend {
    pool: Pool,
    otel_metrics: StorageMetrics,
}

impl DatabaseBackend {
    pub(crate) fn new(pool: Pool, otel_metrics: StorageMetrics) -> Self {
        Self { pool, otel_metrics }
    }

    pub(super) async fn exists(&self, path: &str) -> Result<bool> {
        Ok(sqlx::query_scalar!(
            r#"SELECT COUNT(*) > 0 as "has_count!" FROM files WHERE path = $1"#,
            path
        )
        .fetch_one(&self.pool)
        .await?)
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

        let etag = compute_etag(&content);
        Ok(StreamingBlob {
            path: result.path,
            mime: result
                .mime
                .parse()
                .unwrap_or(mime::APPLICATION_OCTET_STREAM),
            date_updated: result.date_updated,
            etag: Some(etag),
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
