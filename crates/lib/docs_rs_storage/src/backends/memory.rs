use crate::{
    Blob,
    backends::StorageBackendMethods,
    blob::{StreamUpload, StreamingBlob},
    errors::PathNotFoundError,
    metrics::StorageMetrics,
    types::FileRange,
};
use anyhow::{Result, anyhow};
use chrono::Utc;
use dashmap::DashMap;
use docs_rs_headers::compute_etag;
use futures_util::stream::{self, BoxStream};
use itertools::Itertools as _;
use tokio::io;

pub(crate) struct MemoryBackend {
    otel_metrics: StorageMetrics,
    objects: DashMap<String, Blob>,
}

impl MemoryBackend {
    pub(crate) fn new(otel_metrics: StorageMetrics) -> Self {
        Self {
            otel_metrics,
            objects: DashMap::new(),
        }
    }
}

impl StorageBackendMethods for MemoryBackend {
    async fn exists(&self, path: &str) -> Result<bool> {
        Ok(self.objects.contains_key(path))
    }

    async fn get_stream(&self, path: &str, range: Option<FileRange>) -> Result<StreamingBlob> {
        let mut blob = self.objects.get(path).ok_or(PathNotFoundError)?.clone();
        debug_assert!(blob.etag.is_some());

        if let Some(r) = range {
            blob.content = blob
                .content
                .get(*r.start() as usize..=*r.end() as usize)
                .ok_or_else(|| anyhow!("invalid range"))?
                .to_vec();
            blob.etag = Some(compute_etag(&blob.content));
        }
        Ok(blob.into())
    }

    async fn upload_stream(&self, upload: StreamUpload) -> Result<()> {
        let StreamUpload {
            path,
            mime,
            source,
            compression,
        } = upload;

        let mut content = source.reader().await?;
        let mut buffer = Vec::new();
        io::copy(&mut content, &mut buffer).await?;

        let blob = Blob {
            path,
            mime,
            date_updated: Utc::now(),
            etag: Some(compute_etag(&buffer)),
            content: buffer,
            compression,
        };

        self.otel_metrics.uploaded_files.add(1, &[]);
        self.objects.insert(blob.path.clone(), blob);
        Ok(())
    }

    async fn list_prefix<'a>(&'a self, prefix: &'a str) -> BoxStream<'a, Result<String>> {
        Box::pin(stream::iter(
            self.objects
                .iter()
                .filter_map(move |entry| {
                    let key = entry.key();
                    if key.starts_with(prefix) {
                        Some(key.clone())
                    } else {
                        None
                    }
                })
                .sorted_unstable()
                .map(Ok),
        ))
    }

    async fn delete_prefix(&self, prefix: &str) -> Result<()> {
        self.objects.retain(|key, _| !key.starts_with(prefix));
        Ok(())
    }
}
