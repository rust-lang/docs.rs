mod archive_index;
pub(crate) mod compression;
mod database;
mod s3;

pub use self::compression::{CompressionAlgorithm, CompressionAlgorithms, compress, decompress};
use self::{
    compression::{compress_async, wrap_reader_for_decompression},
    database::DatabaseBackend,
    s3::S3Backend,
};
use crate::{
    Config,
    db::{
        BuildId, Pool,
        file::{FileEntry, detect_mime},
        mimes,
        types::version::Version,
    },
    error::Result,
    metrics::otel::AnyMeterProvider,
    utils::spawn_blocking,
};
use anyhow::anyhow;
use axum_extra::headers;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use fn_error_context::context;
use futures_util::stream::BoxStream;
use mime::Mime;
use opentelemetry::metrics::Counter;
use path_slash::PathExt;
use std::{
    fmt,
    fs::{self, File},
    io::{self, BufReader},
    iter,
    num::ParseIntError,
    ops::RangeInclusive,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt},
    runtime,
    sync::Mutex,
    time::sleep,
};
use tracing::{debug, error, info_span, instrument, trace, warn};
use walkdir::WalkDir;

const ARCHIVE_INDEX_FILE_EXTENSION: &str = "index";

type FileRange = RangeInclusive<u64>;

#[derive(Debug, thiserror::Error)]
#[error("path not found")]
pub(crate) struct PathNotFoundError;

/// represents a blob to be uploaded to storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BlobUpload {
    pub(crate) path: String,
    pub(crate) mime: Mime,
    pub(crate) content: Vec<u8>,
    pub(crate) compression: Option<CompressionAlgorithm>,
}

impl From<Blob> for BlobUpload {
    fn from(value: Blob) -> Self {
        Self {
            path: value.path,
            mime: value.mime,
            content: value.content,
            compression: value.compression,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Blob {
    pub(crate) path: String,
    pub(crate) mime: Mime,
    pub(crate) date_updated: DateTime<Utc>,
    pub(crate) etag: Option<headers::ETag>,
    pub(crate) content: Vec<u8>,
    pub(crate) compression: Option<CompressionAlgorithm>,
}

pub(crate) struct StreamingBlob {
    pub(crate) path: String,
    pub(crate) mime: Mime,
    pub(crate) date_updated: DateTime<Utc>,
    pub(crate) etag: Option<headers::ETag>,
    pub(crate) compression: Option<CompressionAlgorithm>,
    pub(crate) content_length: usize,
    pub(crate) content: Box<dyn AsyncBufRead + Unpin + Send>,
}

impl std::fmt::Debug for StreamingBlob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamingBlob")
            .field("path", &self.path)
            .field("mime", &self.mime)
            .field("date_updated", &self.date_updated)
            .field("etag", &self.etag)
            .field("compression", &self.compression)
            .finish()
    }
}

impl StreamingBlob {
    /// wrap the content stream in a streaming decompressor according to the
    /// algorithm found in `compression` attribute.
    pub(crate) async fn decompress(mut self) -> Result<Self, io::Error> {
        let Some(alg) = self.compression else {
            return Ok(self);
        };

        self.content = wrap_reader_for_decompression(self.content, alg);

        // We fill the first bytes here to force the compressor to start decompressing.
        // This is because we want a failure here in this method when the data is corrupted,
        // so we can directly act on that, and users don't have any errors when they just
        // stream the data.
        // This won't _comsume_ the bytes. The user of this StreamingBlob will still be able
        // to stream the whole content.
        //
        // This doesn't work 100% of the time. We might get other i/o error here,
        // or the decompressor might stumble on corrupted data later during streaming.
        //
        // But: the most common error is that the format "magic bytes" at the beginning
        // of the stream are missing, and that's caught here.
        let decompressed_buf = self.content.fill_buf().await?;
        debug_assert!(
            !decompressed_buf.is_empty(),
            "we assume if we have > 0 decompressed bytes, start of the decompression works."
        );

        self.compression = None;
        // not touching the etag, it should represent the original content
        Ok(self)
    }

    /// consume the inner stream and materialize the full blob into memory.
    pub(crate) async fn materialize(mut self, max_size: usize) -> Result<Blob> {
        let mut content = crate::utils::sized_buffer::SizedBuffer::new(max_size);
        content.reserve(self.content_length);

        tokio::io::copy(&mut self.content, &mut content).await?;

        Ok(Blob {
            path: self.path,
            mime: self.mime,
            date_updated: self.date_updated,
            etag: self.etag, // downloading doesn't change the etag
            content: content.into_inner(),
            compression: self.compression,
        })
    }
}

impl From<Blob> for StreamingBlob {
    fn from(value: Blob) -> Self {
        Self {
            path: value.path,
            mime: value.mime,
            date_updated: value.date_updated,
            etag: value.etag,
            compression: value.compression,
            content_length: value.content.len(),
            content: Box::new(io::Cursor::new(value.content)),
        }
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
pub struct InvalidStorageBackendError;

#[derive(Debug)]
pub enum StorageKind {
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

#[derive(Debug)]
struct StorageMetrics {
    uploaded_files: Counter<u64>,
}

impl StorageMetrics {
    fn new(meter_provider: &AnyMeterProvider) -> Self {
        let meter = meter_provider.meter("storage");
        const PREFIX: &str = "docsrs.storage";
        Self {
            uploaded_files: meter
                .u64_counter(format!("{PREFIX}.uploaded_files"))
                .with_unit("1")
                .build(),
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
    /// Locks to synchronize write-access to the locally cached archive index files.
    locks: DashMap<PathBuf, Arc<Mutex<()>>>,
}

impl AsyncStorage {
    pub async fn new(
        pool: Pool,
        config: Arc<Config>,
        otel_meter_provider: &AnyMeterProvider,
    ) -> Result<Self> {
        let otel_metrics = StorageMetrics::new(otel_meter_provider);

        Ok(Self {
            backend: match config.storage_backend {
                StorageKind::Database => {
                    StorageBackend::Database(DatabaseBackend::new(pool, otel_metrics))
                }
                StorageKind::S3 => {
                    StorageBackend::S3(Box::new(S3Backend::new(&config, otel_metrics).await?))
                }
            },
            locks: DashMap::with_capacity(config.local_archive_cache_expected_count),
            config,
        })
    }

    #[instrument]
    pub(crate) async fn exists(&self, path: &str) -> Result<bool> {
        match &self.backend {
            StorageBackend::Database(db) => db.exists(path).await,
            StorageBackend::S3(s3) => s3.exists(path).await,
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
        version: &Version,
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
        version: &Version,
        latest_build_id: Option<BuildId>,
        path: &str,
        archive_storage: bool,
    ) -> Result<Blob> {
        self.stream_source_file(name, version, latest_build_id, path, archive_storage)
            .await?
            .materialize(self.config.max_file_size_for(path))
            .await
    }

    #[instrument]
    pub(crate) async fn stream_source_file(
        &self,
        name: &str,
        version: &Version,
        latest_build_id: Option<BuildId>,
        path: &str,
        archive_storage: bool,
    ) -> Result<StreamingBlob> {
        trace!("fetch source file");
        Ok(if archive_storage {
            self.stream_from_archive(&source_archive_path(name, version), latest_build_id, path)
                .await?
        } else {
            let remote_path = format!("sources/{name}/{version}/{path}");
            self.get_stream(&remote_path).await?
        })
    }

    #[instrument]
    pub(crate) async fn rustdoc_file_exists(
        &self,
        name: &str,
        version: &Version,
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
            .find_in_archive_index(archive_path, latest_build_id, path)
            .await
        {
            Ok(file_info) => Ok(file_info.is_some()),
            Err(err) => {
                if err.downcast_ref::<PathNotFoundError>().is_some() {
                    Ok(false)
                } else {
                    Err(err)
                }
            }
        }
    }

    /// get, decompress and materialize an object from store
    #[instrument]
    pub(crate) async fn get(&self, path: &str, max_size: usize) -> Result<Blob> {
        self.get_stream(path).await?.materialize(max_size).await
    }

    /// get a raw stream to an object in storage
    ///
    /// We don't decompress ourselves, S3 only decompresses with a correct
    /// `Content-Encoding` header set, which we don't.
    #[instrument]
    pub(crate) async fn get_raw_stream(&self, path: &str) -> Result<StreamingBlob> {
        match &self.backend {
            StorageBackend::Database(db) => db.get_stream(path, None).await,
            StorageBackend::S3(s3) => s3.get_stream(path, None).await,
        }
    }

    /// get a decompressing stream to an object in storage.
    #[instrument]
    pub(crate) async fn get_stream(&self, path: &str) -> Result<StreamingBlob> {
        Ok(self.get_raw_stream(path).await?.decompress().await?)
    }

    /// get, decompress and materialize part of an object from store
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

    /// get a decompressing stream to a range inside an object in storage
    #[instrument]
    pub(super) async fn get_range_stream(
        &self,
        path: &str,
        range: FileRange,
        compression: Option<CompressionAlgorithm>,
    ) -> Result<StreamingBlob> {
        let mut raw_stream = match &self.backend {
            StorageBackend::Database(db) => db.get_stream(path, Some(range)).await,
            StorageBackend::S3(s3) => s3.get_stream(path, Some(range)).await,
        }?;
        // `compression` represents the compression of the file-stream inside the archive.
        // We don't compress the whole archive, so the encoding of the archive's blob is irrelevant
        // here.
        raw_stream.compression = compression;
        Ok(raw_stream.decompress().await?)
    }

    fn local_index_cache_lock(&self, local_index_path: impl AsRef<Path>) -> Arc<Mutex<()>> {
        let local_index_path = local_index_path.as_ref().to_path_buf();

        self.locks
            .entry(local_index_path)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .downgrade()
            .clone()
    }

    async fn purge_archive_index_cache(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
    ) -> Result<()> {
        // we know that config.local_archive_cache_path is an absolute path, not relative.
        // So it will be usable as key in the DashMap.
        let local_index_path = self.config.local_archive_cache_path.join(format!(
            "{archive_path}.{}.{ARCHIVE_INDEX_FILE_EXTENSION}",
            latest_build_id.map(|id| id.0).unwrap_or(0)
        ));

        let rwlock = self.local_index_cache_lock(&local_index_path);

        let _write_guard = rwlock.lock().await;

        if tokio::fs::try_exists(&local_index_path).await? {
            tokio::fs::remove_file(&local_index_path).await?;
        }

        Ok(())
    }

    #[instrument(skip(self))]
    async fn download_archive_index(
        &self,
        local_index_path: &Path,
        remote_index_path: &str,
    ) -> Result<()> {
        let parent = local_index_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("index path without parent"))?
            .to_path_buf();
        tokio::fs::create_dir_all(&parent).await?;

        // Create a unique temp file in the cache folder.
        let (temp_file, mut temp_path) = spawn_blocking({
            let folder = self.config.local_archive_cache_path.clone();
            move || -> Result<_> { tempfile::NamedTempFile::new_in(&folder).map_err(Into::into) }
        })
        .await?
        .into_parts();

        // Download into temp file.
        let mut temp_file = tokio::fs::File::from_std(temp_file);
        let mut stream = self.get_stream(remote_index_path).await?.content;
        tokio::io::copy(&mut stream, &mut temp_file).await?;
        temp_file.sync_all().await?;

        temp_path.disable_cleanup(true);

        // Publish atomically.
        // Will replace any existing file.
        tokio::fs::rename(&temp_path, local_index_path).await?;

        // fsync parent dir to make rename durable
        spawn_blocking(move || {
            let dir = std::fs::File::open(parent)?;
            dir.sync_all().map_err(Into::into)
        })
        .await?;

        Ok(())
    }

    /// Find find the file into needed to fetch a certain path inside a remote archive.
    /// Will try to use a local cache of the index file, and otherwise download it
    /// from storage.
    #[instrument(skip(self))]
    async fn find_in_archive_index(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        path_in_archive: &str,
    ) -> Result<Option<archive_index::FileInfo>> {
        // we know that config.local_archive_cache_path is an absolute path, not relative.
        // So it will be usable as key in the DashMap.
        let local_index_path = self.config.local_archive_cache_path.join(format!(
            "{archive_path}.{}.{ARCHIVE_INDEX_FILE_EXTENSION}",
            latest_build_id.map(|id| id.0).unwrap_or(0)
        ));

        // fast path: try to use whatever is there, no locking
        match archive_index::find_in_file(&local_index_path, path_in_archive).await {
            Ok(res) => return Ok(res),
            Err(err) => {
                debug!(?err, "archive index lookup failed, will try repair.");
            }
        }

        let lock = self.local_index_cache_lock(&local_index_path);

        // At this point we know the index is missing or broken.
        // Try to become the "downloader" without queueing as a writer.
        if let Ok(write_guard) = lock.try_lock() {
            // Double-check: maybe someone fixed it between our first failure and now.
            if let Ok(res) = archive_index::find_in_file(&local_index_path, path_in_archive).await {
                return Ok(res);
            }

            let remote_index_path = format!("{archive_path}.{ARCHIVE_INDEX_FILE_EXTENSION}");

            // We are the repairer: download fresh index into place.
            self.download_archive_index(&local_index_path, &remote_index_path)
                .await?;

            // Write lock is dropped here (end of scope), so others can proceed.
            drop(write_guard);

            // Final attempt: if this still fails, bubble the error.
            return archive_index::find_in_file(local_index_path, path_in_archive).await;
        }

        // Someone else is already downloading/repairing. Don't queue on write(); just wait
        // a bit and poll the fast path until it becomes readable or we give up.
        const STEP_MS: u64 = 10;
        const ATTEMPTS: u64 = 50; // = 500ms total wait
        const TOTAL_WAIT_MS: u64 = STEP_MS * ATTEMPTS;

        let mut last_err = None;

        for _ in 0..ATTEMPTS {
            sleep(Duration::from_millis(STEP_MS)).await;

            match archive_index::find_in_file(local_index_path.clone(), path_in_archive).await {
                Ok(res) => return Ok(res),
                Err(err) => {
                    // keep waiting; repair may still be in progress
                    last_err = Some(err);
                }
            }
        }

        // Still not usable after waiting: return the last error we saw.
        Err(last_err
            .unwrap_or_else(|| anyhow!("archive index unavailable after repair wait"))
            .context(format!(
                "no archive index after waiting for {TOTAL_WAIT_MS}ms"
            )))
    }

    #[instrument]
    pub(crate) async fn get_from_archive(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        path: &str,
        max_size: usize,
    ) -> Result<Blob> {
        self.stream_from_archive(archive_path, latest_build_id, path)
            .await?
            .materialize(max_size)
            .await
    }

    #[instrument(skip(self))]
    pub(crate) async fn stream_from_archive(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        path: &str,
    ) -> Result<StreamingBlob> {
        for attempt in 0..2 {
            let info = self
                .find_in_archive_index(archive_path, latest_build_id, path)
                .await?
                .ok_or(PathNotFoundError)?;

            match self
                .get_range_stream(archive_path, info.range(), Some(info.compression()))
                .await
            {
                Ok(stream) => {
                    debug_assert_eq!(stream.compression, None);
                    return Ok(StreamingBlob {
                        path: format!("{archive_path}/{path}"),
                        mime: detect_mime(path),
                        date_updated: stream.date_updated,
                        etag: stream.etag,
                        content: stream.content,
                        content_length: stream.content_length,
                        compression: None,
                    });
                }
                Err(err) if attempt == 0 => {
                    // We have some existing race conditions where the local cache of the index
                    // file is outdated.
                    // These mostly appear as "invalid bzip2 header" errors from the decompression
                    // of the downloaded data, because we're fetching the wrong range from the
                    // archive.
                    // While we're also working on fixing the root causes, we want to have a fallback
                    // here so the user impact is less.
                    // In this case, we purge the locally cached index file and retry once.
                    // We're not checking for the _type_ of error here, which could be improved
                    // in the future, but also doesn't hurt much.
                    //
                    // NOTE: this only works because when creating the stream in `get_stream`, we're
                    // already starting to decompress the first couple of bytes by filling
                    // the BufReader buffer.
                    // The reader of the `StreamingBlob` will still see the full stream.
                    warn!(
                        ?err,
                        "error fetching range from archive, purging local index cache and retrying once"
                    );
                    self.purge_archive_index_cache(archive_path, latest_build_id)
                        .await?;

                    continue;
                }
                Err(err) => return Err(err),
            }
        }

        unreachable!("stream_from_archive retry loop exited unexpectedly");
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
        let remote_index_path = format!("{}.{ARCHIVE_INDEX_FILE_EXTENSION}", &archive_path);
        let compressed_index_content = {
            let _span = info_span!("create_archive_index", %remote_index_path).entered();

            tokio::fs::create_dir_all(&self.config.temp_dir).await?;
            let local_index_path =
                tempfile::NamedTempFile::new_in(&self.config.temp_dir)?.into_temp_path();

            archive_index::create(&mut io::Cursor::new(&mut zip_content), &local_index_path)
                .await?;

            let mut buf: Vec<u8> = Vec::new();
            compress_async(
                &mut tokio::io::BufReader::new(tokio::fs::File::open(&local_index_path).await?),
                &mut buf,
                alg,
            )
            .await?;
            buf
        };

        self.store_inner(vec![
            BlobUpload {
                path: archive_path.to_string(),
                mime: mimes::APPLICATION_ZIP.clone(),
                content: zip_content,
                compression: None,
            },
            BlobUpload {
                path: remote_index_path,
                mime: mime::APPLICATION_OCTET_STREAM,
                content: compressed_index_content,
                compression: Some(alg),
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
                let mut blobs: Vec<BlobUpload> = Vec::new();
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

                    blobs.push(BlobUpload {
                        path: bucket_path,
                        mime,
                        content,
                        compression: Some(alg),
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
    pub(crate) async fn store_blobs(&self, blobs: Vec<BlobUpload>) -> Result<()> {
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

        self.store_inner(vec![BlobUpload {
            path,
            mime,
            content,
            compression: None,
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

        self.store_inner(vec![BlobUpload {
            path,
            mime,
            content,
            compression: Some(alg),
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

        self.store_inner(vec![BlobUpload {
            path: target_path,
            mime,
            content,
            compression: Some(alg),
        }])
        .await?;

        Ok(alg)
    }

    async fn store_inner(&self, batch: Vec<BlobUpload>) -> Result<()> {
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
    runtime: runtime::Handle,
}

#[allow(dead_code)]
impl Storage {
    pub fn new(inner: Arc<AsyncStorage>, runtime: runtime::Handle) -> Self {
        Self { inner, runtime }
    }

    pub(crate) fn exists(&self, path: &str) -> Result<bool> {
        self.runtime.block_on(self.inner.exists(path))
    }

    pub(crate) fn fetch_source_file(
        &self,
        name: &str,
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

    pub(crate) fn rustdoc_file_exists(
        &self,
        name: &str,
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
    pub(crate) fn store_blobs(&self, blobs: Vec<BlobUpload>) -> Result<()> {
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
    pub(crate) async fn cleanup_after_test(&self) -> Result<()> {
        self.inner.cleanup_after_test().await
    }
}

impl std::fmt::Debug for Storage {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "sync wrapper for {:?}", self.inner)
    }
}

pub(crate) fn rustdoc_archive_path(name: &str, version: &Version) -> String {
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
    version: &Version,
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

pub(crate) fn source_archive_path(name: &str, version: &Version) -> String {
    format!("sources/{name}/{version}.zip")
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{test::TestEnvironment, web::headers::compute_etag};
    use std::env;
    use test_case::test_case;

    const ZSTD_EOF_BYTES: [u8; 3] = [0x01, 0x00, 0x00];

    fn streaming_blob(
        content: impl Into<Vec<u8>>,
        alg: Option<CompressionAlgorithm>,
    ) -> StreamingBlob {
        let content = content.into();
        StreamingBlob {
            path: "some_path.db".into(),
            mime: mime::APPLICATION_OCTET_STREAM,
            date_updated: Utc::now(),
            compression: alg,
            etag: Some(compute_etag(&content)),
            content_length: content.len(),
            content: Box::new(io::Cursor::new(content)),
        }
    }

    #[tokio::test]
    async fn test_streaming_blob_uncompressed() -> Result<()> {
        const CONTENT: &[u8] = b"Hello, world!";

        // without decompression
        {
            let stream = streaming_blob(CONTENT, None);
            let blob = stream.materialize(usize::MAX).await?;
            assert_eq!(blob.content, CONTENT);
            assert!(blob.compression.is_none());
        }

        // with decompression, does nothing
        {
            let stream = streaming_blob(CONTENT, None);
            let blob = stream.decompress().await?.materialize(usize::MAX).await?;
            assert_eq!(blob.content, CONTENT);
            assert!(blob.compression.is_none());
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_broken_zstd_blob() -> Result<()> {
        const NOT_ZSTD: &[u8] = b"Hello, world!";
        let alg = CompressionAlgorithm::Zstd;

        // without decompression
        // Doesn't fail because we don't call `.decompress`
        {
            let stream = streaming_blob(NOT_ZSTD, Some(alg));
            let blob = stream.materialize(usize::MAX).await?;
            assert_eq!(blob.content, NOT_ZSTD);
            assert_eq!(blob.compression, Some(alg));
        }

        // with decompression
        // should fail in the `.decompress` call,
        // not later when materializing / streaming.
        {
            let err = streaming_blob(NOT_ZSTD, Some(alg))
                .decompress()
                .await
                .unwrap_err();

            assert_eq!(err.kind(), io::ErrorKind::Other);

            assert_eq!(
                err.to_string(),
                "Unknown frame descriptor",
                "unexpected error: {}",
                err
            );
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_streaming_blob_zstd() -> Result<()> {
        const CONTENT: &[u8] = b"Hello, world!";
        let mut compressed_content = Vec::new();
        let alg = CompressionAlgorithm::Zstd;
        compress_async(
            &mut io::Cursor::new(CONTENT.to_vec()),
            &mut compressed_content,
            alg,
        )
        .await?;

        // without decompression
        {
            let stream = streaming_blob(compressed_content.clone(), Some(alg));
            let blob = stream.materialize(usize::MAX).await?;
            assert_eq!(blob.content, compressed_content);
            assert_eq!(blob.content.last_chunk::<3>().unwrap(), &ZSTD_EOF_BYTES);
            assert_eq!(blob.compression, Some(alg));
        }

        // with decompression
        {
            let blob = streaming_blob(compressed_content.clone(), Some(alg))
                .decompress()
                .await?
                .materialize(usize::MAX)
                .await?;
            assert_eq!(blob.content, CONTENT);
            assert!(blob.compression.is_none());
        }

        Ok(())
    }

    #[tokio::test]
    #[test_case(CompressionAlgorithm::Zstd)]
    #[test_case(CompressionAlgorithm::Bzip2)]
    #[test_case(CompressionAlgorithm::Gzip)]
    async fn test_async_compression(alg: CompressionAlgorithm) -> Result<()> {
        const CONTENT: &[u8] = b"Hello, world! Hello, world! Hello, world! Hello, world!";

        let compressed_index_content = {
            let mut buf: Vec<u8> = Vec::new();
            compress_async(&mut io::Cursor::new(CONTENT.to_vec()), &mut buf, alg).await?;
            buf
        };

        {
            // try low-level async decompression
            let mut decompressed_buf: Vec<u8> = Vec::new();
            let mut reader = wrap_reader_for_decompression(
                io::Cursor::new(compressed_index_content.clone()),
                alg,
            );

            tokio::io::copy(&mut reader, &mut io::Cursor::new(&mut decompressed_buf)).await?;

            assert_eq!(decompressed_buf, CONTENT);
        }

        {
            // try sync decompression
            let decompressed_buf: Vec<u8> = decompress(
                io::Cursor::new(compressed_index_content.clone()),
                alg,
                usize::MAX,
            )?;

            assert_eq!(decompressed_buf, CONTENT);
        }

        // try decompress via storage API
        let blob = StreamingBlob {
            path: "some_path.db".into(),
            mime: mime::APPLICATION_OCTET_STREAM,
            date_updated: Utc::now(),
            etag: None,
            compression: Some(alg),
            content_length: compressed_index_content.len(),
            content: Box::new(io::Cursor::new(compressed_index_content)),
        }
        .decompress()
        .await?
        .materialize(usize::MAX)
        .await?;

        assert_eq!(blob.compression, None);
        assert_eq!(blob.content, CONTENT);

        Ok(())
    }

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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_outdated_local_archive_index_gets_redownloaded() -> Result<()> {
        use tokio::fs;

        let env = TestEnvironment::with_config(
            TestEnvironment::base_config()
                .storage_backend(StorageKind::S3)
                .build()?,
        )
        .await?;

        let storage = env.async_storage();

        // virtual latest build id, used for local caching of the index files
        const LATEST_BUILD_ID: Option<BuildId> = Some(BuildId(42));
        let cache_root = env.config().local_archive_cache_path.clone();

        let cache_filename = |archive_name: &str| {
            cache_root.join(format!(
                "{}.{}.{}",
                archive_name,
                LATEST_BUILD_ID.unwrap(),
                ARCHIVE_INDEX_FILE_EXTENSION
            ))
        };

        /// dummy archives, files will contain their name as content
        async fn create_archive(
            storage: &AsyncStorage,
            archive_name: &str,
            filenames: &[&str],
        ) -> Result<()> {
            let dir = tempfile::Builder::new()
                .prefix("docs.rs-upload-archive-test")
                .tempdir()?;
            for &file in filenames.iter() {
                let path = dir.path().join(file);
                fs::write(path, file).await?;
            }
            storage
                .store_all_in_archive(archive_name, dir.path())
                .await?;

            Ok(())
        }

        // create two archives with indexes that contain the same filename
        create_archive(
            storage,
            "test1.zip",
            &["file1.txt", "file2.txt", "important.txt"],
        )
        .await?;

        create_archive(
            storage,
            "test2.zip",
            &["important.txt", "another_file_1.txt", "another_file_2.txt"],
        )
        .await?;

        for archive_name in &["test1.zip", "test2.zip"] {
            assert!(storage.exists(archive_name).await?);

            assert!(
                storage
                    .exists(&format!("{}.{ARCHIVE_INDEX_FILE_EXTENSION}", archive_name))
                    .await?
            );
            // local index cache doesn't exist yet
            let local_index_file = cache_filename(archive_name);
            assert!(!fs::try_exists(&local_index_file).await?);

            // this will then create the cache
            assert!(
                storage
                    .exists_in_archive(archive_name, LATEST_BUILD_ID, "important.txt")
                    .await?
            );
            assert!(fs::try_exists(&local_index_file).await?);

            // fetching the content out of the archive also works
            assert_eq!(
                storage
                    .get_from_archive(archive_name, LATEST_BUILD_ID, "important.txt", usize::MAX)
                    .await?
                    .content,
                b"important.txt"
            );
        }

        // validate if the positions are really different in the archvies,
        // for the same filename.
        let pos_in_test1_zip = storage
            .find_in_archive_index("test1.zip", LATEST_BUILD_ID, "important.txt")
            .await?
            .unwrap();
        let pos_in_test2_zip = storage
            .find_in_archive_index("test2.zip", LATEST_BUILD_ID, "important.txt")
            .await?
            .unwrap();

        assert_ne!(pos_in_test1_zip.range(), pos_in_test2_zip.range());

        // now I'm swapping the local index files.
        // This should simulate hat I have an outdated byte-range for a file

        let local_index_file_1 = cache_filename("test1.zip");
        let local_index_file_2 = cache_filename("test2.zip");

        {
            let temp_path = cache_root.join("temp_index_swap.tmp");
            fs::rename(&local_index_file_1, &temp_path).await?;
            fs::rename(&local_index_file_2, &local_index_file_1).await?;
            fs::rename(&temp_path, &local_index_file_2).await?;
        }

        // now try to fetch the files inside the archives again, the local files
        // should be removed, refetched, and all should be fine.
        // Without our fallback / delete mechanism, this would fail.

        for archive_name in &["test1.zip", "test2.zip"] {
            assert_eq!(
                storage
                    .get_from_archive(archive_name, LATEST_BUILD_ID, "important.txt", usize::MAX)
                    .await?
                    .content,
                b"important.txt"
            );
        }

        Ok(())
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
    use crate::{test::TestEnvironment, web::headers::compute_etag};

    fn get_file_info(files: &[FileEntry], path: impl AsRef<Path>) -> Option<&FileEntry> {
        let path = path.as_ref();
        files.iter().find(|info| info.path == path)
    }

    fn test_exists(storage: &Storage) -> Result<()> {
        assert!(!storage.exists("path/to/file.txt").unwrap());
        let blob = BlobUpload {
            path: "path/to/file.txt".into(),
            mime: mime::TEXT_PLAIN,
            content: "Hello world!".into(),
            compression: None,
        };
        storage.store_blobs(vec![blob])?;
        assert!(storage.exists("path/to/file.txt")?);

        Ok(())
    }

    fn test_get_object(storage: &Storage) -> Result<()> {
        let path: &str = "foo/bar.txt";
        let blob = BlobUpload {
            path: path.into(),
            mime: mime::TEXT_PLAIN,
            compression: None,
            content: b"test content\n".to_vec(),
        };

        storage.store_blobs(vec![blob.clone()])?;

        let found = storage.get(path, usize::MAX)?;
        assert_eq!(blob.mime, found.mime);
        assert_eq!(blob.content, found.content);
        // while our db backend just does MD5,
        // it seems like minio does it too :)
        assert_eq!(found.etag, Some(compute_etag(&blob.content)));

        for path in &["bar.txt", "baz.txt", "foo/baz.txt"] {
            assert!(
                storage
                    .get(path, usize::MAX)
                    .unwrap_err()
                    .downcast_ref::<PathNotFoundError>()
                    .is_some()
            );
        }

        Ok(())
    }

    fn test_get_range(storage: &Storage) -> Result<()> {
        let blob = BlobUpload {
            path: "foo/bar.txt".into(),
            mime: mime::TEXT_PLAIN,
            compression: None,
            content: b"test content\n".to_vec(),
        };

        let full_etag = compute_etag(&blob.content);

        storage.store_blobs(vec![blob.clone()])?;

        let mut etags = Vec::new();

        for range in [0..=4, 5..=12] {
            let partial_blob = storage.get_range("foo/bar.txt", usize::MAX, range.clone(), None)?;
            let range = (*range.start() as usize)..=(*range.end() as usize);
            assert_eq!(blob.content[range], partial_blob.content);

            etags.push(partial_blob.etag.unwrap());
        }
        if let [etag1, etag2] = &etags[..] {
            assert_ne!(etag1, etag2);
            assert_ne!(etag1, &full_etag);
            assert_ne!(etag2, &full_etag);
        } else {
            panic!("expected two etags");
        }

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
                .map(|&filename| BlobUpload {
                    path: filename.into(),
                    mime: mime::TEXT_PLAIN,
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

        let small_blob = BlobUpload {
            path: "small-blob.bin".into(),
            mime: mime::TEXT_PLAIN,
            content: vec![0; MAX_SIZE],
            compression: None,
        };
        let big_blob = BlobUpload {
            path: "big-blob.bin".into(),
            mime: mime::TEXT_PLAIN,
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

    fn test_store_blobs(env: &TestEnvironment, storage: &Storage) -> Result<()> {
        const NAMES: &[&str] = &[
            "a",
            "b",
            "a_very_long_file_name_that_has_an.extension",
            "parent/child",
            "h/i/g/h/l/y/_/n/e/s/t/e/d/_/d/i/r/e/c/t/o/r/i/e/s",
        ];

        let blobs = NAMES
            .iter()
            .map(|&path| BlobUpload {
                path: path.into(),
                mime: mime::TEXT_PLAIN,
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

        let collected_metrics = env.collected_metrics();

        assert_eq!(
            collected_metrics
                .get_metric("storage", "docsrs.storage.uploaded_files")?
                .get_u64_counter()
                .value(),
            NAMES.len() as u64,
        );

        Ok(())
    }

    fn test_exists_without_remote_archive(storage: &Storage) -> Result<()> {
        // when remote and local index don't exist, any `exists_in_archive`  should
        // return `false`
        assert!(!storage.exists_in_archive("some_archive_name", None, "some_file_name")?);
        Ok(())
    }

    fn test_store_all_in_archive(env: &TestEnvironment, storage: &Storage) -> Result<()> {
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
            .join(format!("folder/test.zip.0.{ARCHIVE_INDEX_FILE_EXTENSION}"));

        let (stored_files, compression_alg) =
            storage.store_all_in_archive("folder/test.zip", dir.path())?;

        assert!(storage.exists(&format!("folder/test.zip.{ARCHIVE_INDEX_FILE_EXTENSION}"))?);

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

        let collected_metrics = env.collected_metrics();

        assert_eq!(
            collected_metrics
                .get_metric("storage", "docsrs.storage.uploaded_files")?
                .get_u64_counter()
                .value(),
            2,
        );

        Ok(())
    }

    fn test_store_all(env: &TestEnvironment, storage: &Storage) -> Result<()> {
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

        let collected_metrics = env.collected_metrics();
        assert_eq!(
            collected_metrics
                .get_metric("storage", "docsrs.storage.uploaded_files")?
                .get_u64_counter()
                .value(),
            2,
        );

        Ok(())
    }

    fn test_batched_uploads(storage: &Storage) -> Result<()> {
        let uploads: Vec<_> = (0..=100)
            .map(|i| {
                let content = format!("const IDX: usize = {i};").as_bytes().to_vec();
                BlobUpload {
                    mime: mimes::TEXT_RUST.clone(),
                    content,
                    path: format!("{i}.rs"),
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
                .map(|path| BlobUpload {
                    path: (*path).to_string(),
                    content: b"foo\n".to_vec(),
                    compression: None,
                    mime: mime::TEXT_PLAIN,
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
                    use crate::storage::{StorageKind};

                    fn get_env() -> anyhow::Result<crate::test::TestEnvironment> {
                        crate::test::TestEnvironment::with_config_and_runtime(
                            TestEnvironment::base_config()
                                .storage_backend($config)
                                .build()?
                        )
                    }

                    backend_tests!(@tests $tests);
                    backend_tests!(@tests_with_metrics $tests_with_metrics);
                }
            )*
        };
        (@tests { $($test:ident,)* }) => {
            $(
                #[test]
                fn $test() -> anyhow::Result<()> {
                    let env = get_env()?;
                    super::$test(&*env.storage())
                }
            )*
        };
        (@tests_with_metrics { $($test:ident,)* }) => {
            $(
                #[test]
                fn $test() -> anyhow::Result<()> {
                    let env = get_env()?;
                    super::$test(&env, &*env.storage())
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
        }

        tests_with_metrics {
            test_store_blobs,
            test_store_all,
            test_store_all_in_archive,
        }
    }
}
