use crate::{blob::Blob, file::FileEntry, storage::non_blocking::AsyncStorage, types::FileRange};
use anyhow::Result;
use docs_rs_types::{BuildId, CompressionAlgorithm, KrateName, Version};
use std::{fmt, path::Path, sync::Arc};
use tokio::runtime;

/// Sync wrapper around `AsyncStorage` for parts of the codebase that are not async.
pub struct Storage {
    inner: Arc<AsyncStorage>,
    runtime: runtime::Handle,
}

#[allow(dead_code)]
impl Storage {
    pub fn new(inner: Arc<AsyncStorage>, runtime: runtime::Handle) -> Self {
        Self { inner, runtime }
    }

    pub fn exists(&self, path: &str) -> Result<bool> {
        self.runtime.block_on(self.inner.exists(path))
    }

    pub fn fetch_source_file(
        &self,
        name: &KrateName,
        version: &Version,
        latest_build_id: Option<BuildId>,
        path: &str,
        archive_storage: bool,
    ) -> Result<Blob> {
        self.runtime.block_on(self.inner.fetch_source_file(
            name,
            version,
            latest_build_id,
            path,
            archive_storage,
        ))
    }

    pub fn rustdoc_file_exists(
        &self,
        name: &KrateName,
        version: &Version,
        latest_build_id: Option<BuildId>,
        path: &str,
        archive_storage: bool,
    ) -> Result<bool> {
        self.runtime.block_on(self.inner.rustdoc_file_exists(
            name,
            version,
            latest_build_id,
            path,
            archive_storage,
        ))
    }

    pub fn exists_in_archive(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        path: &str,
    ) -> Result<bool> {
        self.runtime.block_on(
            self.inner
                .exists_in_archive(archive_path, latest_build_id, path),
        )
    }

    pub fn get(&self, path: &str, max_size: usize) -> Result<Blob> {
        self.runtime.block_on(self.inner.get(path, max_size))
    }

    pub(crate) fn get_range(
        &self,
        path: &str,
        max_size: usize,
        range: FileRange,
        compression: Option<CompressionAlgorithm>,
    ) -> Result<Blob> {
        self.runtime
            .block_on(self.inner.get_range(path, max_size, range, compression))
    }

    pub fn get_from_archive(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        path: &str,
        max_size: usize,
    ) -> Result<Blob> {
        self.runtime.block_on(self.inner.get_from_archive(
            archive_path,
            latest_build_id,
            path,
            max_size,
        ))
    }

    pub fn store_all_in_archive(
        &self,
        archive_path: &str,
        root_dir: &Path,
    ) -> Result<(Vec<FileEntry>, CompressionAlgorithm)> {
        self.runtime
            .block_on(self.inner.store_all_in_archive(archive_path, root_dir))
    }

    pub fn store_all(
        &self,
        prefix: &Path,
        root_dir: &Path,
    ) -> Result<(Vec<FileEntry>, CompressionAlgorithm)> {
        self.runtime
            .block_on(self.inner.store_all(prefix, root_dir))
    }

    #[cfg(test)]
    pub fn store_blobs(&self, blobs: Vec<crate::blob::BlobUpload>) -> Result<()> {
        self.runtime.block_on(self.inner.store_blobs(blobs))
    }

    // Store file into the backend at the given path, uncompressed.
    // The path will also be used to determine the mime type.
    pub fn store_one_uncompressed(
        &self,
        path: impl Into<String> + std::fmt::Debug,
        content: impl Into<Vec<u8>>,
    ) -> Result<()> {
        self.runtime
            .block_on(self.inner.store_one_uncompressed(path, content))
    }

    // Store file into the backend at the given path (also used to detect mime type), returns the
    // chosen compression algorithm
    pub fn store_one(
        &self,
        path: impl Into<String> + std::fmt::Debug,
        content: impl Into<Vec<u8>>,
    ) -> Result<CompressionAlgorithm> {
        self.runtime.block_on(self.inner.store_one(path, content))
    }

    // Store file into the backend at the given path (also used to detect mime type), returns the
    // chosen compression algorithm
    pub fn store_path(
        &self,
        target_path: impl Into<String> + std::fmt::Debug,
        source_path: impl AsRef<Path> + std::fmt::Debug,
    ) -> Result<CompressionAlgorithm> {
        self.runtime
            .block_on(self.inner.store_path(target_path, source_path))
    }

    /// sync wrapper for the list_prefix function
    /// purely for testing purposes since it collects all files into a Vec.
    #[cfg(feature = "testing")]
    pub fn list_prefix(&self, prefix: &str) -> impl Iterator<Item = Result<String>> {
        use futures_util::stream::StreamExt;
        self.runtime
            .block_on(async {
                self.inner
                    .list_prefix(prefix)
                    .await
                    .collect::<Vec<_>>()
                    .await
            })
            .into_iter()
    }

    pub fn delete_prefix(&self, prefix: &str) -> Result<()> {
        self.runtime.block_on(self.inner.delete_prefix(prefix))
    }
}

impl std::fmt::Debug for Storage {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "sync wrapper for {:?}", self.inner)
    }
}
