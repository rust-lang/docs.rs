mod archive_index;
pub(crate) mod compression;
mod database;
mod s3;

pub use self::compression::{CompressionAlgorithm, CompressionAlgorithms, compress, decompress};
use self::compression::{wrap_reader_for_decompression, wrap_writer_for_compression};
use self::database::DatabaseBackend;
use self::s3::S3Backend;
use crate::{
    Config, InstanceMetrics,
    db::{
        BuildId, Pool,
        file::{FileEntry, detect_mime},
        mimes,
    },
    error::Result,
    utils::spawn_blocking,
};
use anyhow::anyhow;
use chrono::{DateTime, Utc};
use fn_error_context::context;
use futures_util::stream::BoxStream;
use mime::Mime;
use path_slash::PathExt;
use std::{
    fmt,
    fs::{self, File},
    io::{self, BufReader},
    num::ParseIntError,
    ops::RangeInclusive,
    path::{Path, PathBuf},
    sync::Arc,
};
use std::{iter, str::FromStr};
use tokio::{
    io::{AsyncRead, AsyncWriteExt},
    runtime::Runtime,
};
use tracing::{error, info_span, instrument, trace};
use walkdir::WalkDir;

type FileRange = RangeInclusive<u64>;

#[derive(Debug, thiserror::Error)]
#[error("path not found")]
pub(crate) struct PathNotFoundError;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Blob {
    pub(crate) path: String,
    pub(crate) mime: Mime,
    pub(crate) date_updated: DateTime<Utc>,
    pub(crate) content: Vec<u8>,
    pub(crate) compression: Option<CompressionAlgorithm>,
}

impl Blob {
    pub(crate) fn is_empty(&self) -> bool {
        self.mime == "application/x-empty"
    }
}

pub(crate) struct StreamingBlob {
    pub(crate) path: String,
    pub(crate) mime: Mime,
    pub(crate) date_updated: DateTime<Utc>,
    pub(crate) compression: Option<CompressionAlgorithm>,
    pub(crate) content_length: usize,
    pub(crate) content: Box<dyn AsyncRead + Unpin + Send>,
}

impl std::fmt::Debug for StreamingBlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamingBlob")
            .field("path", &self.path)
            .field("mime", &self.mime)
            .field("date_updated", &self.date_updated)
            .field("compression", &self.compression)
            .finish()
    }
}

impl StreamingBlob {
    /// wrap the content stream in a streaming decompressor according to the
    /// algorithm found in `compression` attribute.
    pub(crate) fn decompress(mut self) -> Self {
        let Some(alg) = self.compression else {
            return self;
        };

        self.content = wrap_reader_for_decompression(self.content, alg);
        self.compression = None;
        self
    }

    pub(crate) async fn materialize(mut self, max_size: usize) -> Result<Blob> {
        self = self.decompress();

        let mut content = crate::utils::sized_buffer::SizedBuffer::new(max_size);
        content.reserve(self.content_length);

        tokio::io::copy(&mut self.content, &mut content).await?;

        Ok(Blob {
            path: self.path,
            mime: self.mime,
            date_updated: self.date_updated,
            content: content.into_inner(),
            compression: self.compression,
        })
    }
}

pub fn get_file_list<P: AsRef<Path>>(path: P) -> Box<dyn Iterator<Item = Result<PathBuf>>> {
    let path = path.as_ref().to_path_buf();
    if path.is_file() {
        let path = if let Some(parent) = path.parent() {
            path.strip_prefix(parent).unwrap().to_path_buf()
        } else {
            path
        };

        Box::new(iter::once(Ok(path)))
    } else if path.is_dir() {
        Box::new(
            WalkDir::new(path.clone())
                .into_iter()
                .filter_map(move |result| {
                    let direntry = match result {
                        Ok(de) => de,
                        Err(err) => return Some(Err(err.into())),
                    };

                    if !direntry.file_type().is_dir() {
                        Some(Ok(direntry
                            .path()
                            .strip_prefix(&path)
                            .unwrap()
                            .to_path_buf()))
                    } else {
                        None
                    }
                }),
        )
    } else {
        Box::new(iter::empty())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid storage backend")]
pub(crate) struct InvalidStorageBackendError;

#[derive(Debug)]
pub(crate) enum StorageKind {
    Database,
    S3,
}

impl std::str::FromStr for StorageKind {
    type Err = InvalidStorageBackendError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        match input {
            "database" => Ok(StorageKind::Database),
            "s3" => Ok(StorageKind::S3),
            _ => Err(InvalidStorageBackendError),
        }
    }
}

enum StorageBackend {
    Database(DatabaseBackend),
    S3(Box<S3Backend>),
}

pub struct AsyncStorage {
    backend: StorageBackend,
    config: Arc<Config>,
}

impl AsyncStorage {
    pub async fn new(
        pool: Pool,
        metrics: Arc<InstanceMetrics>,
        config: Arc<Config>,
    ) -> Result<Self> {
        Ok(Self {
            config: config.clone(),
            backend: match config.storage_backend {
                StorageKind::Database => {
                    StorageBackend::Database(DatabaseBackend::new(pool, metrics))
                }
                StorageKind::S3 => {
                    StorageBackend::S3(Box::new(S3Backend::new(metrics, &config).await?))
                }
            },
        })
    }

    #[instrument]
    pub(crate) async fn exists(&self, path: &str) -> Result<bool> {
        match &self.backend {
            StorageBackend::Database(db) => db.exists(path).await,
            StorageBackend::S3(s3) => s3.exists(path).await,
        }
    }

    #[instrument]
    pub(crate) async fn get_public_access(&self, path: &str) -> Result<bool> {
        match &self.backend {
            StorageBackend::Database(db) => db.get_public_access(path).await,
            StorageBackend::S3(s3) => s3.get_public_access(path).await,
        }
    }

    #[instrument]
    pub(crate) async fn set_public_access(&self, path: &str, public: bool) -> Result<()> {
        match &self.backend {
            StorageBackend::Database(db) => db.set_public_access(path, public).await,
            StorageBackend::S3(s3) => s3.set_public_access(path, public).await,
        }
    }

    fn max_file_size_for(&self, path: &str) -> usize {
        if path.ends_with(".html") {
            self.config.max_file_size_html
        } else {
            self.config.max_file_size
        }
    }

    /// Fetch a rustdoc file from our blob storage.
    /// * `name` - the crate name
    /// * `version` - the crate version
    /// * `latest_build_id` - the id of the most recent build. used purely to invalidate the local archive
    ///   index cache, when `archive_storage` is `true.` Without it we wouldn't know that we have
    ///   to invalidate the locally cached file after a rebuild.
    /// * `path` - the wanted path inside the documentation.
    /// * `archive_storage` - if `true`, we will assume we have a remove ZIP archive and an index
    ///    where we can fetch the requested path from inside the ZIP file.
    #[instrument]
    pub(crate) async fn stream_rustdoc_file(
        &self,
        name: &str,
        version: &str,
        latest_build_id: Option<BuildId>,
        path: &str,
        archive_storage: bool,
    ) -> Result<StreamingBlob> {
        trace!("fetch rustdoc file");
        Ok(if archive_storage {
            self.stream_from_archive(&rustdoc_archive_path(name, version), latest_build_id, path)
                .await?
        } else {
            // Add rustdoc prefix, name and version to the path for accessing the file stored in the database
            let remote_path = format!("rustdoc/{name}/{version}/{path}");
            self.get_stream(&remote_path).await?
        })
    }

    #[context("fetching {path} from {name} {version} (archive: {archive_storage})")]
    pub(crate) async fn fetch_source_file(
        &self,
        name: &str,
        version: &str,
        latest_build_id: Option<BuildId>,
        path: &str,
        archive_storage: bool,
    ) -> Result<Blob> {
        Ok(if archive_storage {
            self.get_from_archive(
                &source_archive_path(name, version),
                latest_build_id,
                path,
                self.max_file_size_for(path),
            )
            .await?
        } else {
            let remote_path = format!("sources/{name}/{version}/{path}");
            self.get(&remote_path, self.max_file_size_for(path)).await?
        })
    }

    #[instrument]
    pub(crate) async fn rustdoc_file_exists(
        &self,
        name: &str,
        version: &str,
        latest_build_id: Option<BuildId>,
        path: &str,
        archive_storage: bool,
    ) -> Result<bool> {
        Ok(if archive_storage {
            self.exists_in_archive(&rustdoc_archive_path(name, version), latest_build_id, path)
                .await?
        } else {
            // Add rustdoc prefix, name and version to the path for accessing the file stored in the database
            let remote_path = format!("rustdoc/{name}/{version}/{path}");
            self.exists(&remote_path).await?
        })
    }

    #[instrument]
    pub(crate) async fn exists_in_archive(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        path: &str,
    ) -> Result<bool> {
        match self
            .download_archive_index(archive_path, latest_build_id)
            .await
        {
            Ok(index_filename) => Ok(archive_index::find_in_file(index_filename, path)
                .await?
                .is_some()),
            Err(err) => {
                if err.downcast_ref::<PathNotFoundError>().is_some() {
                    Ok(false)
                } else {
                    Err(err)
                }
            }
        }
    }

    #[instrument]
    pub(crate) async fn get(&self, path: &str, max_size: usize) -> Result<Blob> {
        self.get_stream(path).await?.materialize(max_size).await
    }

    #[instrument]
    pub(crate) async fn get_stream(&self, path: &str) -> Result<StreamingBlob> {
        let blob = match &self.backend {
            StorageBackend::Database(db) => db.get_stream(path, None).await,
            StorageBackend::S3(s3) => s3.get_stream(path, None).await,
        }?;
        Ok(blob.decompress())
    }

    #[instrument]
    pub(super) async fn get_range(
        &self,
        path: &str,
        max_size: usize,
        range: FileRange,
        compression: Option<CompressionAlgorithm>,
    ) -> Result<Blob> {
        self.get_range_stream(path, range, compression)
            .await?
            .materialize(max_size)
            .await
    }

    #[instrument]
    pub(super) async fn get_range_stream(
        &self,
        path: &str,
        range: FileRange,
        compression: Option<CompressionAlgorithm>,
    ) -> Result<StreamingBlob> {
        let mut blob = match &self.backend {
            StorageBackend::Database(db) => db.get_stream(path, Some(range)).await,
            StorageBackend::S3(s3) => s3.get_stream(path, Some(range)).await,
        }?;
        // `compression` represents the compression of the file-stream inside the archive.
        // We don't compress the whole archive, so the encoding of the archive's blob is irrelevant
        // here.
        blob.compression = compression;
        Ok(blob.decompress())
    }

    #[instrument]
    pub(super) async fn download_archive_index(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
    ) -> Result<PathBuf> {
        // remote/folder/and/x.zip.index
        let remote_index_path = format!("{archive_path}.index");
        let local_index_path = self.config.local_archive_cache_path.join(format!(
            "{archive_path}.{}.index",
            latest_build_id.map(|id| id.0).unwrap_or(0)
        ));

        if !local_index_path.exists() {
            let index_content = self.get(&remote_index_path, usize::MAX).await?.content;

            tokio::fs::create_dir_all(
                local_index_path
                    .parent()
                    .ok_or_else(|| anyhow!("index path without parent"))?,
            )
            .await?;

            // when we don't have a locally cached index and many parallel request
            // we might download the same archive index multiple times here.
            // So we're storing the content into a temporary file before renaming it
            // into the final location.
            let temp_path = tempfile::NamedTempFile::new_in(&self.config.local_archive_cache_path)?
                .into_temp_path();
            let mut file = tokio::fs::File::create(&temp_path).await?;
            file.write_all(&index_content).await?;
            tokio::fs::rename(temp_path, &local_index_path).await?;
        }

        Ok(local_index_path)
    }

    #[instrument]
    pub(crate) async fn get_from_archive(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        path: &str,
        max_size: usize,
    ) -> Result<Blob> {
        let index_filename = self
            .download_archive_index(archive_path, latest_build_id)
            .await?;

        let info = archive_index::find_in_file(index_filename, path)
            .await?
            .ok_or(PathNotFoundError)?;

        let blob = self
            .get_range(
                archive_path,
                max_size,
                info.range(),
                Some(info.compression()),
            )
            .await?;
        assert_eq!(blob.compression, None);

        Ok(Blob {
            path: format!("{archive_path}/{path}"),
            mime: detect_mime(path),
            date_updated: blob.date_updated,
            content: blob.content,
            compression: None,
        })
    }

    #[instrument]
    pub(crate) async fn stream_from_archive(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        path: &str,
    ) -> Result<StreamingBlob> {
        let index_filename = self
            .download_archive_index(archive_path, latest_build_id)
            .await?;

        let info = archive_index::find_in_file(index_filename, path)
            .await?
            .ok_or(PathNotFoundError)?;

        let blob = self
            .get_range_stream(archive_path, info.range(), Some(info.compression()))
            .await?;
        assert_eq!(blob.compression, None);

        Ok(StreamingBlob {
            path: format!("{archive_path}/{path}"),
            mime: detect_mime(path),
            date_updated: blob.date_updated,
            content: blob.content,
            content_length: blob.content_length,
            compression: None,
        })
    }

    #[instrument(skip(self))]
    pub(crate) async fn store_all_in_archive(
        &self,
        archive_path: &str,
        root_dir: &Path,
    ) -> Result<(Vec<FileEntry>, CompressionAlgorithm)> {
        let (mut zip_content,    file_paths) =
            spawn_blocking({
                let archive_path = archive_path.to_owned();
                let root_dir = root_dir.to_owned();

                move || {
                    let mut file_paths = Vec::new();

                    // We are only using the `zip` library to create the archives and the matching
                    // index-file. The ZIP format allows more compression formats, and these can even be mixed
                    // in a single archive.
                    //
                    // Decompression happens by fetching only the part of the remote archive that contains
                    // the compressed stream of the object we put into the archive.
                    // For decompression we are sharing the compression algorithms defined in
                    // `storage::compression`. So every new algorithm to be used inside ZIP archives
                    // also has to be added as supported algorithm for storage compression, together
                    // with a mapping in `storage::archive_index::Index::new_from_zip`.

                    let zip_content = {
                        let _span =
                            info_span!("create_zip_archive", %archive_path, root_dir=%root_dir.display()).entered();

                        let options = zip::write::SimpleFileOptions::default()
                            .compression_method(zip::CompressionMethod::Bzip2);

                        let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
                        for file_path in get_file_list(&root_dir) {
                            let file_path = file_path?;

                            let mut file = fs::File::open(root_dir.join(&file_path))?;
                            zip.start_file(file_path.to_str().unwrap(), options)?;
                            io::copy(&mut file, &mut zip)?;
                            file_paths.push(FileEntry{path: file_path, size: file.metadata()?.len()});
                        }

                        zip.finish()?.into_inner()
                    };

                    Ok((
                        zip_content,
                        file_paths
                    ))
                }
            })
            .await?;

        let alg = CompressionAlgorithm::default();
        let remote_index_path = format!("{}.index", &archive_path);
        let compressed_index_content = {
            let _span = info_span!("create_archive_index", %remote_index_path).entered();

            tokio::fs::create_dir_all(&self.config.temp_dir).await?;
            let local_index_path =
                tempfile::NamedTempFile::new_in(&self.config.temp_dir)?.into_temp_path();

            archive_index::create(&mut io::Cursor::new(&mut zip_content), &local_index_path)
                .await?;

            let mut buf: Vec<u8> = Vec::new();
            tokio::io::copy(
                &mut tokio::io::BufReader::new(tokio::fs::File::open(&local_index_path).await?),
                &mut wrap_writer_for_compression(&mut buf, alg),
            )
            .await?;
            buf
        };

        self.store_inner(vec![
            Blob {
                path: archive_path.to_string(),
                mime: mimes::APPLICATION_ZIP.clone(),
                content: zip_content,
                compression: None,
                date_updated: Utc::now(),
            },
            Blob {
                path: remote_index_path,
                mime: mime::APPLICATION_OCTET_STREAM,
                content: compressed_index_content,
                compression: Some(alg),
                date_updated: Utc::now(),
            },
        ])
        .await?;

        Ok((file_paths, CompressionAlgorithm::Bzip2))
    }

    /// Store all files in `root_dir` into the backend under `prefix`.
    #[instrument(skip(self))]
    pub(crate) async fn store_all(
        &self,
        prefix: &Path,
        root_dir: &Path,
    ) -> Result<(Vec<FileEntry>, CompressionAlgorithm)> {
        let alg = CompressionAlgorithm::default();

        let (blobs, file_paths_and_mimes) = spawn_blocking({
            let prefix = prefix.to_owned();
            let root_dir = root_dir.to_owned();
            move || {
                let mut file_paths = Vec::new();
                let mut blobs: Vec<Blob> = Vec::new();
                for file_path in get_file_list(&root_dir) {
                    let file_path = file_path?;

                    // Some files have insufficient permissions
                    // (like .lock file created by cargo in documentation directory).
                    // Skip these files.
                    let Ok(file) = fs::File::open(root_dir.join(&file_path)) else {
                        continue;
                    };

                    let file_size = file.metadata()?.len();

                    let content = compress(file, alg)?;
                    let bucket_path = prefix.join(&file_path).to_slash().unwrap().to_string();

                    let file_info = FileEntry {
                        path: file_path,
                        size: file_size,
                    };
                    let mime = file_info.mime();
                    file_paths.push(file_info);

                    blobs.push(Blob {
                        path: bucket_path,
                        mime,
                        content,
                        compression: Some(alg),
                        // this field is ignored by the backend
                        date_updated: Utc::now(),
                    });
                }
                Ok((blobs, file_paths))
            }
        })
        .await?;

        self.store_inner(blobs).await?;
        Ok((file_paths_and_mimes, alg))
    }

    #[cfg(test)]
    pub(crate) async fn store_blobs(&self, blobs: Vec<Blob>) -> Result<()> {
        self.store_inner(blobs).await
    }

    // Store file into the backend at the given path, uncompressed.
    // The path will also be used to determine the mime type.
    #[instrument(skip(self, content))]
    pub(crate) async fn store_one_uncompressed(
        &self,
        path: impl Into<String> + std::fmt::Debug,
        content: impl Into<Vec<u8>>,
    ) -> Result<()> {
        let path = path.into();
        let content = content.into();
        let mime = detect_mime(&path).to_owned();

        self.store_inner(vec![Blob {
            path,
            mime,
            content,
            compression: None,
            // this field is ignored by the backend
            date_updated: Utc::now(),
        }])
        .await?;

        Ok(())
    }

    // Store file into the backend at the given path (also used to detect mime type), returns the
    // chosen compression algorithm
    #[instrument(skip(self, content))]
    pub(crate) async fn store_one(
        &self,
        path: impl Into<String> + std::fmt::Debug,
        content: impl Into<Vec<u8>>,
    ) -> Result<CompressionAlgorithm> {
        let path = path.into();
        let content = content.into();
        let alg = CompressionAlgorithm::default();
        let content = compress(&*content, alg)?;
        let mime = detect_mime(&path).to_owned();

        self.store_inner(vec![Blob {
            path,
            mime,
            content,
            compression: Some(alg),
            // this field is ignored by the backend
            date_updated: Utc::now(),
        }])
        .await?;

        Ok(alg)
    }

    #[instrument(skip(self))]
    pub(crate) async fn store_path(
        &self,
        target_path: impl Into<String> + std::fmt::Debug,
        source_path: impl AsRef<Path> + std::fmt::Debug,
    ) -> Result<CompressionAlgorithm> {
        let target_path = target_path.into();
        let source_path = source_path.as_ref();

        let alg = CompressionAlgorithm::default();
        let content = compress(BufReader::new(File::open(source_path)?), alg)?;

        let mime = detect_mime(&target_path).to_owned();

        self.store_inner(vec![Blob {
            path: target_path,
            mime,
            content,
            compression: Some(alg),
            // this field is ignored by the backend
            date_updated: Utc::now(),
        }])
        .await?;

        Ok(alg)
    }

    async fn store_inner(&self, batch: Vec<Blob>) -> Result<()> {
        match &self.backend {
            StorageBackend::Database(db) => db.store_batch(batch).await,
            StorageBackend::S3(s3) => s3.store_batch(batch).await,
        }
    }

    pub(super) async fn list_prefix<'a>(
        &'a self,
        prefix: &'a str,
    ) -> BoxStream<'a, Result<String>> {
        match &self.backend {
            StorageBackend::Database(db) => Box::pin(db.list_prefix(prefix).await),
            StorageBackend::S3(s3) => Box::pin(s3.list_prefix(prefix).await),
        }
    }

    pub(crate) async fn delete_prefix(&self, prefix: &str) -> Result<()> {
        match &self.backend {
            StorageBackend::Database(db) => db.delete_prefix(prefix).await,
            StorageBackend::S3(s3) => s3.delete_prefix(prefix).await,
        }
    }

    // We're using `&self` instead of consuming `self` or creating a Drop impl because during tests
    // we leak the web server, and Drop isn't executed in that case (since the leaked web server
    // still holds a reference to the storage).
    #[cfg(test)]
    pub(crate) async fn cleanup_after_test(&self) -> Result<()> {
        if let StorageBackend::S3(s3) = &self.backend {
            s3.cleanup_after_test().await?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for AsyncStorage {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.backend {
            StorageBackend::Database(_) => write!(f, "database-backed storage"),
            StorageBackend::S3(_) => write!(f, "S3-backed storage"),
        }
    }
}

/// Sync wrapper around `AsyncStorage` for parts of the codebase that are not async.
pub struct Storage {
    inner: Arc<AsyncStorage>,
    runtime: Arc<Runtime>,
}

#[allow(dead_code)]
impl Storage {
    pub fn new(inner: Arc<AsyncStorage>, runtime: Arc<Runtime>) -> Self {
        Self { inner, runtime }
    }

    pub(crate) fn exists(&self, path: &str) -> Result<bool> {
        self.runtime.block_on(self.inner.exists(path))
    }

    pub(crate) fn get_public_access(&self, path: &str) -> Result<bool> {
        self.runtime.block_on(self.inner.get_public_access(path))
    }

    pub(crate) fn set_public_access(&self, path: &str, public: bool) -> Result<()> {
        self.runtime
            .block_on(self.inner.set_public_access(path, public))
    }

    pub(crate) fn fetch_source_file(
        &self,
        name: &str,
        version: &str,
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

    pub(crate) fn rustdoc_file_exists(
        &self,
        name: &str,
        version: &str,
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

    pub(crate) fn exists_in_archive(
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

    pub(crate) fn get(&self, path: &str, max_size: usize) -> Result<Blob> {
        self.runtime.block_on(self.inner.get(path, max_size))
    }

    pub(super) fn get_range(
        &self,
        path: &str,
        max_size: usize,
        range: FileRange,
        compression: Option<CompressionAlgorithm>,
    ) -> Result<Blob> {
        self.runtime
            .block_on(self.inner.get_range(path, max_size, range, compression))
    }

    pub(super) fn download_index(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
    ) -> Result<PathBuf> {
        self.runtime.block_on(
            self.inner
                .download_archive_index(archive_path, latest_build_id),
        )
    }

    pub(crate) fn get_from_archive(
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

    pub(crate) fn store_all_in_archive(
        &self,
        archive_path: &str,
        root_dir: &Path,
    ) -> Result<(Vec<FileEntry>, CompressionAlgorithm)> {
        self.runtime
            .block_on(self.inner.store_all_in_archive(archive_path, root_dir))
    }

    pub(crate) fn store_all(
        &self,
        prefix: &Path,
        root_dir: &Path,
    ) -> Result<(Vec<FileEntry>, CompressionAlgorithm)> {
        self.runtime
            .block_on(self.inner.store_all(prefix, root_dir))
    }

    #[cfg(test)]
    pub(crate) fn store_blobs(&self, blobs: Vec<Blob>) -> Result<()> {
        self.runtime.block_on(self.inner.store_blobs(blobs))
    }

    // Store file into the backend at the given path, uncompressed.
    // The path will also be used to determine the mime type.
    #[instrument(skip(self, content))]
    pub(crate) fn store_one_uncompressed(
        &self,
        path: impl Into<String> + std::fmt::Debug,
        content: impl Into<Vec<u8>>,
    ) -> Result<()> {
        self.runtime
            .block_on(self.inner.store_one_uncompressed(path, content))
    }

    // Store file into the backend at the given path (also used to detect mime type), returns the
    // chosen compression algorithm
    #[instrument(skip(self, content))]
    pub(crate) fn store_one(
        &self,
        path: impl Into<String> + std::fmt::Debug,
        content: impl Into<Vec<u8>>,
    ) -> Result<CompressionAlgorithm> {
        self.runtime.block_on(self.inner.store_one(path, content))
    }

    // Store file into the backend at the given path (also used to detect mime type), returns the
    // chosen compression algorithm
    #[instrument(skip(self))]
    pub(crate) fn store_path(
        &self,
        target_path: impl Into<String> + std::fmt::Debug,
        source_path: impl AsRef<Path> + std::fmt::Debug,
    ) -> Result<CompressionAlgorithm> {
        self.runtime
            .block_on(self.inner.store_path(target_path, source_path))
    }

    /// sync wrapper for the list_prefix function
    /// purely for testing purposes since it collects all files into a Vec.
    #[cfg(test)]
    pub(crate) fn list_prefix(&self, prefix: &str) -> impl Iterator<Item = Result<String>> {
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

    #[instrument(skip(self))]
    pub(crate) fn delete_prefix(&self, prefix: &str) -> Result<()> {
        self.runtime.block_on(self.inner.delete_prefix(prefix))
    }

    // We're using `&self` instead of consuming `self` or creating a Drop impl because during tests
    // we leak the web server, and Drop isn't executed in that case (since the leaked web server
    // still holds a reference to the storage).
    #[cfg(test)]
    pub(crate) fn cleanup_after_test(&self) -> Result<()> {
        self.runtime.block_on(self.inner.cleanup_after_test())
    }
}

impl std::fmt::Debug for Storage {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "sync wrapper for {:?}", self.inner)
    }
}

pub(crate) fn rustdoc_archive_path(name: &str, version: &str) -> String {
    format!("rustdoc/{name}/{version}.zip")
}

#[derive(strum::Display, Debug, PartialEq, Eq, Clone, Copy)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum RustdocJsonFormatVersion {
    #[strum(serialize = "{0}")]
    Version(u16),
    Latest,
}

impl FromStr for RustdocJsonFormatVersion {
    type Err = ParseIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "latest" {
            Ok(RustdocJsonFormatVersion::Latest)
        } else {
            s.parse::<u16>().map(RustdocJsonFormatVersion::Version)
        }
    }
}

pub(crate) fn rustdoc_json_path(
    name: &str,
    version: &str,
    target: &str,
    format_version: RustdocJsonFormatVersion,
    compression_algorithm: Option<CompressionAlgorithm>,
) -> String {
    let mut path = format!(
        "rustdoc-json/{name}/{version}/{target}/{name}_{version}_{target}_{format_version}.json"
    );

    if let Some(alg) = compression_algorithm {
        path.push('.');
        path.push_str(compression::file_extension_for(alg));
    }

    path
}

pub(crate) fn source_archive_path(name: &str, version: &str) -> String {
    format!("sources/{name}/{version}.zip")
}

#[cfg(test)]
mod test {
    use super::*;
    use std::env;
    use test_case::test_case;

    #[test_case("latest", RustdocJsonFormatVersion::Latest)]
    #[test_case("42", RustdocJsonFormatVersion::Version(42))]
    fn test_json_format_version(input: &str, expected: RustdocJsonFormatVersion) {
        // test Display
        assert_eq!(expected.to_string(), input);
        // test FromStr
        assert_eq!(expected, input.parse().unwrap());
    }

    #[test]
    fn test_get_file_list() -> Result<()> {
        crate::test::init_logger();
        let dir = env::current_dir().unwrap();

        let files: Vec<_> = get_file_list(&dir).collect::<Result<Vec<_>>>()?;
        assert!(!files.is_empty());

        let files: Vec<_> = get_file_list(dir.join("Cargo.toml")).collect::<Result<Vec<_>>>()?;
        assert_eq!(files[0], std::path::Path::new("Cargo.toml"));

        Ok(())
    }

    #[test]
    fn test_mime_types() {
        check_mime(".gitignore", "text/plain");
        check_mime("hello.toml", "text/toml");
        check_mime("hello.css", "text/css");
        check_mime("hello.js", "text/javascript");
        check_mime("hello.html", "text/html");
        check_mime("hello.hello.md", "text/markdown");
        check_mime("hello.markdown", "text/markdown");
        check_mime("hello.json", "application/json");
        check_mime("hello.txt", "text/plain");
        check_mime("file.rs", "text/rust");
        check_mime("important.svg", "image/svg+xml");
    }

    fn check_mime(path: &str, expected_mime: &str) {
        let detected_mime = detect_mime(Path::new(&path));
        assert_eq!(detected_mime, expected_mime);
    }
}

/// Backend tests are a set of tests executed on all the supported storage backends. They ensure
/// docs.rs behaves the same no matter the storage backend currently used.
///
/// To add a new test create the function without adding the `#[test]` attribute, and add the
/// function name to the `backend_tests!` macro at the bottom of the module.
///
/// This is the preferred way to test whether backends work.
#[cfg(test)]
mod backend_tests {
    use super::*;

    fn get_file_info(files: &[FileEntry], path: impl AsRef<Path>) -> Option<&FileEntry> {
        let path = path.as_ref();
        files.iter().find(|info| info.path == path)
    }

    fn test_exists(storage: &Storage) -> Result<()> {
        assert!(!storage.exists("path/to/file.txt").unwrap());
        let blob = Blob {
            path: "path/to/file.txt".into(),
            mime: mime::TEXT_PLAIN,
            date_updated: Utc::now(),
            content: "Hello world!".into(),
            compression: None,
        };
        storage.store_blobs(vec![blob])?;
        assert!(storage.exists("path/to/file.txt")?);

        Ok(())
    }

    fn test_set_public(storage: &Storage) -> Result<()> {
        let path: &str = "foo/bar.txt";

        storage.store_blobs(vec![Blob {
            path: path.into(),
            mime: mime::TEXT_PLAIN,
            date_updated: Utc::now(),
            compression: None,
            content: b"test content\n".to_vec(),
        }])?;

        assert!(!storage.get_public_access(path)?);
        storage.set_public_access(path, true)?;
        assert!(storage.get_public_access(path)?);
        storage.set_public_access(path, false)?;
        assert!(!storage.get_public_access(path)?);

        for path in &["bar.txt", "baz.txt", "foo/baz.txt"] {
            assert!(
                storage
                    .set_public_access(path, true)
                    .unwrap_err()
                    .downcast_ref::<PathNotFoundError>()
                    .is_some()
            );
        }

        Ok(())
    }

    fn test_get_object(storage: &Storage) -> Result<()> {
        let path: &str = "foo/bar.txt";
        let blob = Blob {
            path: path.into(),
            mime: mime::TEXT_PLAIN,
            date_updated: Utc::now(),
            compression: None,
            content: b"test content\n".to_vec(),
        };

        storage.store_blobs(vec![blob.clone()])?;

        let found = storage.get(path, usize::MAX)?;
        assert_eq!(blob.mime, found.mime);
        assert_eq!(blob.content, found.content);

        // default visibility is private
        assert!(!storage.get_public_access(path)?);

        for path in &["bar.txt", "baz.txt", "foo/baz.txt"] {
            assert!(
                storage
                    .get(path, usize::MAX)
                    .unwrap_err()
                    .downcast_ref::<PathNotFoundError>()
                    .is_some()
            );

            assert!(
                storage
                    .get_public_access(path)
                    .unwrap_err()
                    .downcast_ref::<PathNotFoundError>()
                    .is_some()
            );
        }

        Ok(())
    }

    fn test_get_range(storage: &Storage) -> Result<()> {
        let blob = Blob {
            path: "foo/bar.txt".into(),
            mime: mime::TEXT_PLAIN,
            date_updated: Utc::now(),
            compression: None,
            content: b"test content\n".to_vec(),
        };

        storage.store_blobs(vec![blob.clone()])?;

        assert_eq!(
            blob.content[0..=4],
            storage
                .get_range("foo/bar.txt", usize::MAX, 0..=4, None)?
                .content
        );
        assert_eq!(
            blob.content[5..=12],
            storage
                .get_range("foo/bar.txt", usize::MAX, 5..=12, None)?
                .content
        );

        for path in &["bar.txt", "baz.txt", "foo/baz.txt"] {
            assert!(
                storage
                    .get_range(path, usize::MAX, 0..=4, None)
                    .unwrap_err()
                    .downcast_ref::<PathNotFoundError>()
                    .is_some()
            );
        }

        Ok(())
    }

    fn test_list_prefix(storage: &Storage) -> Result<()> {
        static FILENAMES: &[&str] = &["baz.txt", "some/bar.txt"];

        storage.store_blobs(
            FILENAMES
                .iter()
                .map(|&filename| Blob {
                    path: filename.into(),
                    mime: mime::TEXT_PLAIN,
                    date_updated: Utc::now(),
                    compression: None,
                    content: b"test content\n".to_vec(),
                })
                .collect(),
        )?;

        assert_eq!(
            storage.list_prefix("").collect::<Result<Vec<String>>>()?,
            FILENAMES
        );

        assert_eq!(
            storage
                .list_prefix("some/")
                .collect::<Result<Vec<String>>>()?,
            &["some/bar.txt"]
        );

        Ok(())
    }

    fn test_too_long_filename(storage: &Storage) -> Result<()> {
        // minio returns ErrKeyTooLongError when the key is over 1024 bytes long.
        // When testing, minio just gave me `XMinioInvalidObjectName`, so I'll check that too.
        let long_filename = "ATCG".repeat(512);

        assert!(
            storage
                .get(&long_filename, 42)
                .unwrap_err()
                .is::<PathNotFoundError>()
        );

        Ok(())
    }

    fn test_get_too_big(storage: &Storage) -> Result<()> {
        const MAX_SIZE: usize = 1024;

        let small_blob = Blob {
            path: "small-blob.bin".into(),
            mime: mime::TEXT_PLAIN,
            date_updated: Utc::now(),
            content: vec![0; MAX_SIZE],
            compression: None,
        };
        let big_blob = Blob {
            path: "big-blob.bin".into(),
            mime: mime::TEXT_PLAIN,
            date_updated: Utc::now(),
            content: vec![0; MAX_SIZE * 2],
            compression: None,
        };

        storage.store_blobs(vec![small_blob.clone(), big_blob])?;

        let blob = storage.get("small-blob.bin", MAX_SIZE)?;
        assert_eq!(blob.content.len(), small_blob.content.len());

        assert!(
            storage
                .get("big-blob.bin", MAX_SIZE)
                .unwrap_err()
                .downcast_ref::<std::io::Error>()
                .and_then(|io| io.get_ref())
                .and_then(|err| err.downcast_ref::<crate::error::SizeLimitReached>())
                .is_some()
        );

        Ok(())
    }

    fn test_store_blobs(storage: &Storage, metrics: &InstanceMetrics) -> Result<()> {
        const NAMES: &[&str] = &[
            "a",
            "b",
            "a_very_long_file_name_that_has_an.extension",
            "parent/child",
            "h/i/g/h/l/y/_/n/e/s/t/e/d/_/d/i/r/e/c/t/o/r/i/e/s",
        ];

        let blobs = NAMES
            .iter()
            .map(|&path| Blob {
                path: path.into(),
                mime: mime::TEXT_PLAIN,
                date_updated: Utc::now(),
                compression: None,
                content: b"Hello world!\n".to_vec(),
            })
            .collect::<Vec<_>>();

        storage.store_blobs(blobs.clone()).unwrap();

        for blob in &blobs {
            let actual = storage.get(&blob.path, usize::MAX)?;
            assert_eq!(blob.path, actual.path);
            assert_eq!(blob.mime, actual.mime);
        }

        assert_eq!(NAMES.len(), metrics.uploaded_files_total.get() as usize);

        Ok(())
    }

    fn test_exists_without_remote_archive(storage: &Storage) -> Result<()> {
        // when remote and local index don't exist, any `exists_in_archive`  should
        // return `false`
        assert!(!storage.exists_in_archive("some_archive_name", None, "some_file_name")?);
        Ok(())
    }

    fn test_store_all_in_archive(storage: &Storage, metrics: &InstanceMetrics) -> Result<()> {
        let dir = tempfile::Builder::new()
            .prefix("docs.rs-upload-archive-test")
            .tempdir()?;
        let files = ["Cargo.toml", "src/main.rs"];
        for &file in &files {
            let path = dir.path().join(file);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, "data")?;
        }

        let local_index_location = storage
            .inner
            .config
            .local_archive_cache_path
            .join("folder/test.zip.0.index");

        let (stored_files, compression_alg) =
            storage.store_all_in_archive("folder/test.zip", dir.path())?;

        assert!(storage.exists("folder/test.zip.index")?);

        assert_eq!(compression_alg, CompressionAlgorithm::Bzip2);
        assert_eq!(stored_files.len(), files.len());
        for name in &files {
            assert!(get_file_info(&stored_files, name).is_some());
        }
        assert_eq!(
            get_file_info(&stored_files, "Cargo.toml").unwrap().mime(),
            "text/toml"
        );
        assert_eq!(
            get_file_info(&stored_files, "src/main.rs").unwrap().mime(),
            "text/rust"
        );

        // delete the existing index to test the download of it
        if local_index_location.exists() {
            fs::remove_file(&local_index_location)?;
        }

        // the first exists-query will download and store the index
        assert!(!local_index_location.exists());
        assert!(storage.exists_in_archive("folder/test.zip", None, "Cargo.toml",)?);

        // the second one will use the local index
        assert!(local_index_location.exists());
        assert!(storage.exists_in_archive("folder/test.zip", None, "src/main.rs",)?);

        let file = storage.get_from_archive("folder/test.zip", None, "Cargo.toml", usize::MAX)?;
        assert_eq!(file.content, b"data");
        assert_eq!(file.mime, "text/toml");
        assert_eq!(file.path, "folder/test.zip/Cargo.toml");

        let file = storage.get_from_archive("folder/test.zip", None, "src/main.rs", usize::MAX)?;
        assert_eq!(file.content, b"data");
        assert_eq!(file.mime, "text/rust");
        assert_eq!(file.path, "folder/test.zip/src/main.rs");

        assert_eq!(2, metrics.uploaded_files_total.get());

        Ok(())
    }

    fn test_store_all(storage: &Storage, metrics: &InstanceMetrics) -> Result<()> {
        let dir = tempfile::Builder::new()
            .prefix("docs.rs-upload-test")
            .tempdir()?;
        let files = ["Cargo.toml", "src/main.rs"];
        for &file in &files {
            let path = dir.path().join(file);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, "data")?;
        }

        let (stored_files, algs) = storage.store_all(Path::new("prefix"), dir.path())?;
        assert_eq!(stored_files.len(), files.len());
        for name in &files {
            assert!(get_file_info(&stored_files, name).is_some());
        }
        assert_eq!(
            get_file_info(&stored_files, "Cargo.toml").unwrap().mime(),
            "text/toml"
        );
        assert_eq!(
            get_file_info(&stored_files, "src/main.rs").unwrap().mime(),
            "text/rust"
        );

        let file = storage.get("prefix/Cargo.toml", usize::MAX)?;
        assert_eq!(file.content, b"data");
        assert_eq!(file.mime, "text/toml");
        assert_eq!(file.path, "prefix/Cargo.toml");

        let file = storage.get("prefix/src/main.rs", usize::MAX)?;
        assert_eq!(file.content, b"data");
        assert_eq!(file.mime, "text/rust");
        assert_eq!(file.path, "prefix/src/main.rs");

        assert_eq!(algs, CompressionAlgorithm::default());

        assert_eq!(2, metrics.uploaded_files_total.get());

        Ok(())
    }

    fn test_batched_uploads(storage: &Storage) -> Result<()> {
        let now = Utc::now();
        let uploads: Vec<_> = (0..=100)
            .map(|i| {
                let content = format!("const IDX: usize = {i};").as_bytes().to_vec();
                Blob {
                    mime: mimes::TEXT_RUST.clone(),
                    content,
                    path: format!("{i}.rs"),
                    date_updated: now,
                    compression: None,
                }
            })
            .collect();

        storage.store_blobs(uploads.clone())?;

        for blob in &uploads {
            let stored = storage.get(&blob.path, usize::MAX)?;
            assert_eq!(&stored.content, &blob.content);
        }

        Ok(())
    }

    fn test_delete_prefix_without_matches(storage: &Storage) -> Result<()> {
        storage.delete_prefix("prefix_without_objects")
    }

    fn test_delete_prefix(storage: &Storage) -> Result<()> {
        test_deletion(
            storage,
            "foo/bar/",
            &[
                "foo.txt",
                "foo/bar.txt",
                "foo/bar/baz.txt",
                "foo/bar/foobar.txt",
                "bar.txt",
            ],
            &["foo.txt", "foo/bar.txt", "bar.txt"],
            &["foo/bar/baz.txt", "foo/bar/foobar.txt"],
        )
    }

    fn test_delete_percent(storage: &Storage) -> Result<()> {
        // PostgreSQL treats "%" as a special char when deleting a prefix. Make sure any "%" in the
        // provided prefix is properly escaped.
        test_deletion(
            storage,
            "foo/%/",
            &["foo/bar.txt", "foo/%/bar.txt"],
            &["foo/bar.txt"],
            &["foo/%/bar.txt"],
        )
    }

    fn test_deletion(
        storage: &Storage,
        prefix: &str,
        start: &[&str],
        present: &[&str],
        missing: &[&str],
    ) -> Result<()> {
        storage.store_blobs(
            start
                .iter()
                .map(|path| Blob {
                    path: (*path).to_string(),
                    content: b"foo\n".to_vec(),
                    compression: None,
                    mime: mime::TEXT_PLAIN,
                    date_updated: Utc::now(),
                })
                .collect(),
        )?;

        storage.delete_prefix(prefix)?;

        for existing in present {
            assert!(storage.get(existing, usize::MAX).is_ok());
        }
        for missing in missing {
            assert!(
                storage
                    .get(missing, usize::MAX)
                    .unwrap_err()
                    .downcast_ref::<PathNotFoundError>()
                    .is_some()
            );
        }

        Ok(())
    }

    // Remember to add the test name to the macro below when adding a new one.

    macro_rules! backend_tests {
        (
            backends { $($backend:ident => $config:expr,)* }
            tests $tests:tt
            tests_with_metrics $tests_with_metrics:tt
        ) => {
            $(
                mod $backend {
                    use crate::test::TestEnvironment;
                    use crate::storage::{Storage, StorageKind};
                    use std::sync::Arc;

                    fn get_storage(env: &TestEnvironment) -> Arc<Storage> {
                        env.override_config(|config| {
                            config.storage_backend = $config;
                        });
                        env.storage()
                    }

                    backend_tests!(@tests $tests);
                    backend_tests!(@tests_with_metrics $tests_with_metrics);
                }
            )*
        };
        (@tests { $($test:ident,)* }) => {
            $(
                #[test]
                fn $test() {
                    crate::test::wrapper(|env| {
                        super::$test(&*get_storage(env))
                    });
                }
            )*
        };
        (@tests_with_metrics { $($test:ident,)* }) => {
            $(
                #[test]
                fn $test() {
                    crate::test::wrapper(|env| {
                        super::$test(&*get_storage(env), &*env.instance_metrics())
                    });
                }
            )*
        };
    }

    backend_tests! {
        backends {
            s3 => StorageKind::S3,
            database => StorageKind::Database,
        }

        tests {
            test_batched_uploads,
            test_exists,
            test_get_object,
            test_get_range,
            test_get_too_big,
            test_too_long_filename,
            test_list_prefix,
            test_delete_prefix,
            test_delete_prefix_without_matches,
            test_delete_percent,
            test_exists_without_remote_archive,
            test_set_public,
        }

        tests_with_metrics {
            test_store_blobs,
            test_store_all,
            test_store_all_in_archive,
        }
    }
}
