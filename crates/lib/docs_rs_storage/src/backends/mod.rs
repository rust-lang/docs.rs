#[cfg(any(test, feature = "testing"))]
pub(crate) mod memory;
pub(crate) mod s3;

use crate::{BlobUpload, StreamingBlob, types::FileRange};
use anyhow::Result;
use futures_util::stream::BoxStream;

pub(crate) trait StorageBackendMethods {
    async fn exists(&self, path: &str) -> Result<bool>;
    async fn get_stream(&self, path: &str, range: Option<FileRange>) -> Result<StreamingBlob>;
    async fn store_batch(&self, batch: Vec<BlobUpload>) -> Result<()>;
    async fn list_prefix<'a>(&'a self, prefix: &'a str) -> BoxStream<'a, Result<String>>;
    async fn delete_prefix(&self, prefix: &str) -> Result<()>;
}

pub(crate) enum StorageBackend {
    #[cfg(any(test, feature = "testing"))]
    Memory(memory::MemoryBackend),
    S3(s3::S3Backend),
}

macro_rules! call_inner {
    ($self:expr, $method:ident ( $($args:expr),* $(,)? )) => {{
        match $self {
            #[cfg(any(test, feature = "testing"))]
            StorageBackend::Memory(backend) => backend.$method($($args),*).await,
            StorageBackend::S3(backend) => backend.$method($($args),*).await,
        }
    }};
}

impl StorageBackendMethods for StorageBackend {
    async fn exists(&self, path: &str) -> Result<bool> {
        call_inner!(self, exists(path))
    }

    async fn get_stream(&self, path: &str, range: Option<FileRange>) -> Result<StreamingBlob> {
        call_inner!(self, get_stream(path, range))
    }

    async fn store_batch(&self, batch: Vec<BlobUpload>) -> Result<()> {
        call_inner!(self, store_batch(batch))
    }

    async fn list_prefix<'a>(&'a self, prefix: &'a str) -> BoxStream<'a, Result<String>> {
        call_inner!(self, list_prefix(prefix))
    }

    async fn delete_prefix(&self, prefix: &str) -> Result<()> {
        call_inner!(self, delete_prefix(prefix))
    }
}
