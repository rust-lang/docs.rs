#[cfg(any(test, feature = "testing"))]
use crate::backends::memory::MemoryBackend;
use crate::{
    Config,
    archive_index::{self, ARCHIVE_INDEX_FILE_EXTENSION, Index},
    backends::{StorageBackend, StorageBackendMethods, s3::S3Backend},
    blob::{Blob, StreamUpload, StreamUploadSource, StreamingBlob},
    compression::{compress, compress_async},
    errors::PathNotFoundError,
    file::FileEntry,
    metrics::StorageMetrics,
    types::{FileRange, StorageKind},
    utils::{
        file_list::{get_file_list, walk_dir_recursive},
        storage_path::{rustdoc_archive_path, source_archive_path},
    },
};
use anyhow::{Context as _, Result};
use docs_rs_mimes::{self as mimes, detect_mime};
use docs_rs_opentelemetry::AnyMeterProvider;
use docs_rs_types::{BuildId, CompressionAlgorithm, KrateName, Version};
use docs_rs_utils::spawn_blocking;
use futures_util::{TryStreamExt as _, future, stream::BoxStream};
use std::{
    fmt,
    io::{Cursor, Write as _},
    path::Path,
    pin::Pin,
    sync::Arc,
};
use tokio::{fs, io, io::AsyncWriteExt as _};
use tokio_util::bytes::Bytes;
use tracing::{info_span, instrument, trace, warn};

/// buffer size when writing zip files.
pub(crate) const ZIP_BUFFER_SIZE: usize = 1024 * 1024;

pub struct AsyncStorage {
    backend: StorageBackend,
    config: Arc<Config>,
    archive_index_cache: archive_index::Cache,
}

impl AsyncStorage {
    pub async fn new(config: Arc<Config>, otel_meter_provider: &AnyMeterProvider) -> Result<Self> {
        let metrics = StorageMetrics::new(otel_meter_provider);

        Ok(Self {
            archive_index_cache: archive_index::Cache::new(
                config.archive_index_cache.clone(),
                otel_meter_provider,
            )
            .await
            .context("initialize archive index cache")?,
            backend: match config.storage_backend {
                #[cfg(any(test, feature = "testing"))]
                StorageKind::Memory => StorageBackend::Memory(MemoryBackend::new(metrics)),
                StorageKind::S3 => StorageBackend::S3(S3Backend::new(&config, metrics).await?),
            },
            config,
        })
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub async fn find_archive_index(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
    ) -> Result<Index> {
        self.archive_index_cache
            .find_index(archive_path, latest_build_id, self)
            .await
    }

    #[instrument(skip(self))]
    pub async fn exists(&self, path: &str) -> Result<bool> {
        self.backend.exists(path).await
    }

    /// Fetch a rustdoc file from our blob storage.
    /// * `name` - the crate name
    /// * `version` - the crate version
    /// * `latest_build_id` - the id of the most recent build. used purely to invalidate the local archive
    ///   index cache. Without it we wouldn't know that we have
    ///   to invalidate the locally cached file after a rebuild.
    /// * `path` - the wanted path inside the documentation.
    #[instrument(skip(self))]
    pub async fn stream_rustdoc_file(
        &self,
        name: &KrateName,
        version: &Version,
        latest_build_id: Option<BuildId>,
        path: &str,
    ) -> Result<StreamingBlob> {
        trace!("fetch rustdoc file");
        self.stream_from_archive(&rustdoc_archive_path(name, version), latest_build_id, path)
            .await
    }

    #[instrument(skip(self))]
    pub async fn fetch_source_file(
        &self,
        name: &KrateName,
        version: &Version,
        latest_build_id: Option<BuildId>,
        path: &str,
    ) -> Result<Blob> {
        self.stream_source_file(name, version, latest_build_id, path)
            .await?
            .materialize(self.config.max_file_size_for(path))
            .await
    }

    #[instrument(skip(self))]
    pub async fn stream_source_file(
        &self,
        name: &KrateName,
        version: &Version,
        latest_build_id: Option<BuildId>,
        path: &str,
    ) -> Result<StreamingBlob> {
        trace!("fetch source file");
        self.stream_from_archive(&source_archive_path(name, version), latest_build_id, path)
            .await
    }

    #[instrument(skip(self))]
    pub async fn rustdoc_file_exists(
        &self,
        name: &KrateName,
        version: &Version,
        latest_build_id: Option<BuildId>,
        path: &str,
    ) -> Result<bool> {
        self.exists_in_archive(&rustdoc_archive_path(name, version), latest_build_id, path)
            .await
    }

    #[instrument(skip(self))]
    pub async fn exists_in_archive(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        path: &str,
    ) -> Result<bool> {
        match self
            .archive_index_cache
            .find(archive_path, latest_build_id, path, self)
            .await
        {
            Ok(file_info) => Ok(file_info.is_some()),
            Err(err) if err.downcast_ref::<PathNotFoundError>().is_some() => Ok(false),
            Err(err) => Err(err),
        }
    }

    /// get, decompress and materialize an object from store
    #[instrument(skip(self))]
    pub async fn get(&self, path: &str, max_size: usize) -> Result<Blob> {
        self.get_stream(path).await?.materialize(max_size).await
    }

    /// get a raw stream to an object in storage
    ///
    /// We don't decompress ourselves, S3 only decompresses with a correct
    /// `Content-Encoding` header set, which we don't.
    #[instrument(skip(self))]
    pub async fn get_raw_stream(&self, path: &str) -> Result<StreamingBlob> {
        self.backend.get_stream(path, None).await
    }

    /// get a decompressing stream to an object in storage.
    #[instrument(skip(self))]
    pub async fn get_stream(&self, path: &str) -> Result<StreamingBlob> {
        Ok(self.get_raw_stream(path).await?.decompress().await?)
    }

    // #[cfg(test)]
    // pub(crate) async fn get_range(
    //     &self,
    //     path: &str,
    //     max_size: usize,
    //     range: FileRange,
    //     compression: Option<CompressionAlgorithm>,
    // ) -> Result<Blob> {
    //     self.get_range_stream(path, range, compression)
    //         .await?
    //         .materialize(max_size)
    //         .await
    // }

    /// get a decompressing stream to a range inside an object in storage
    #[instrument(skip(self))]
    pub(crate) async fn get_range_stream(
        &self,
        path: &str,
        range: FileRange,
        compression: Option<CompressionAlgorithm>,
    ) -> Result<StreamingBlob> {
        let mut raw_stream = self.backend.get_stream(path, Some(range)).await?;
        // `compression` represents the compression of the file-stream inside the archive.
        // We don't compress the whole archive, so the encoding of the archive's blob is irrelevant
        // here.
        raw_stream.compression = compression;
        Ok(raw_stream.decompress().await?)
    }

    #[cfg(test)]
    async fn get_from_archive(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        path: &str,
    ) -> Result<Blob> {
        self.stream_from_archive(archive_path, latest_build_id, path)
            .await?
            .materialize(self.config.max_file_size_for(path))
            .await
    }

    #[instrument(skip(self))]
    pub async fn stream_from_archive(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        path: &str,
    ) -> Result<StreamingBlob> {
        for attempt in 0..2 {
            let info = self
                .archive_index_cache
                .find(archive_path, latest_build_id, path, self)
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
                    self.archive_index_cache
                        .purge(archive_path, latest_build_id)
                        .await?;

                    continue;
                }
                Err(err) => return Err(err),
            }
        }

        unreachable!("stream_from_archive retry loop exited unexpectedly");
    }

    #[instrument(skip(self))]
    pub async fn store_all_in_archive(
        &self,
        archive_path: &str,
        root_dir: impl AsRef<Path> + fmt::Debug,
    ) -> Result<(Vec<FileEntry>, CompressionAlgorithm)> {
        let root_dir = root_dir.as_ref();

        // Keep the TempPath guards alive until after both uploads complete; dropping them earlier
        // would delete the files while S3 is still reading from them.
        let zip_temp_path = tempfile::NamedTempFile::new()?.into_temp_path();
        let zip_path = zip_temp_path.to_path_buf();

        let file_paths =
            spawn_blocking({
                use std::{io, fs};
                let archive_path = archive_path.to_owned();
                let root_dir = root_dir.to_owned();
                let zip_path = zip_path.clone();

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


                    {
                        let _span =
                            info_span!("create_zip_archive", %archive_path, root_dir=%root_dir.display()).entered();

                        let options = zip::write::SimpleFileOptions::default()
                            .compression_method(zip::CompressionMethod::Deflated)
                            .compression_level(Some(6));

                        // rustdoc archives can become a couple of GiB big, so we better use a tempfile.
                        let zip_file = fs::File::create(&zip_path)?;
                        let mut zip = zip::ZipWriter::new(io::BufWriter::with_capacity(ZIP_BUFFER_SIZE, zip_file));
                        for file_path in get_file_list(&root_dir) {
                            let file_path = file_path?;

                            let mut file = fs::File::open(root_dir.join(&file_path))?;
                            zip.start_file(file_path.to_str().unwrap(), options)?;
                            io::copy(&mut file, &mut zip)?;
                            file_paths.push(FileEntry{path: file_path, size: file.metadata()?.len()});
                        }

                        let mut zip_file = zip.finish()?.into_inner()?;
                        zip_file.flush()?;
                    }

                    Ok(file_paths)
                }
            })
            .await?;

        let alg = CompressionAlgorithm::default();
        let remote_index_path = format!("{}.{ARCHIVE_INDEX_FILE_EXTENSION}", archive_path);
        let compressed_index_temp_path = tempfile::NamedTempFile::new()?.into_temp_path();
        let compressed_index_path = compressed_index_temp_path.to_path_buf();
        {
            let _span = info_span!("create_archive_index", %remote_index_path).entered();

            let local_index_path = tempfile::NamedTempFile::new()?.into_temp_path();

            archive_index::create(
                io::BufReader::new(fs::File::open(&zip_path).await?),
                &local_index_path,
            )
            .await?;

            // compressed index can become up to a couple 100 MiB big, so rather use a tempfile.
            let mut compressed_index_file = fs::File::create(&compressed_index_path).await?;
            {
                let mut compressed_index_writer = io::BufWriter::new(&mut compressed_index_file);
                compress_async(
                    &mut io::BufReader::new(fs::File::open(&local_index_path).await?),
                    &mut compressed_index_writer,
                    alg,
                )
                .await?;
                compressed_index_writer.flush().await?;
            }
        }

        tokio::try_join!(
            self.backend.upload_stream(StreamUpload {
                path: archive_path.to_string(),
                mime: mimes::APPLICATION_ZIP.clone(),
                source: StreamUploadSource::File(zip_path),
                compression: None,
            }),
            self.backend.upload_stream(StreamUpload {
                path: remote_index_path,
                mime: mime::APPLICATION_OCTET_STREAM,
                source: StreamUploadSource::File(compressed_index_path),
                compression: Some(alg),
            })
        )?;

        Ok((file_paths, CompressionAlgorithm::Deflate))
    }

    /// Store all files in `root_dir` into the backend under `prefix`.
    #[instrument(skip(self))]
    pub async fn store_all(
        &self,
        prefix: impl AsRef<Path> + fmt::Debug,
        root_dir: impl AsRef<Path> + fmt::Debug,
    ) -> Result<(Vec<FileEntry>, CompressionAlgorithm)> {
        let prefix = prefix.as_ref().to_path_buf();
        let root_dir = root_dir.as_ref();
        let alg = CompressionAlgorithm::default();

        let file_paths_and_mimes: Vec<_> = walk_dir_recursive(&root_dir)
            .err_into::<anyhow::Error>()
            .map_ok(|item| {
                let prefix = prefix.clone();
                async move {
                    // Some files have insufficient permissions
                    // (like .lock file created by cargo in documentation directory).
                    // Skip these files.
                    let Ok(file) = fs::File::open(&item).await else {
                        return Ok(None);
                    };

                    let content = {
                        let mut buf: Vec<u8> = Vec::new();
                        compress_async(io::BufReader::new(file), &mut buf, alg).await?;
                        buf
                    };

                    let bucket_path = prefix.join(&item.relative).to_string_lossy().to_string();

                    let file_size = item.metadata.len();

                    let file_info = FileEntry {
                        path: item.relative.clone(),
                        size: file_size,
                    };
                    let mime = file_info.mime().clone();

                    self.backend
                        .upload_stream(StreamUpload {
                            path: bucket_path,
                            mime,
                            source: StreamUploadSource::Bytes(content.into()),
                            compression: Some(alg),
                        })
                        .await?;

                    Ok(Some(file_info))
                }
            })
            .try_buffer_unordered(self.config.network_parallelism)
            .try_filter_map(|item| future::ready(Ok(item)))
            .try_collect()
            .await?;

        Ok((file_paths_and_mimes, alg))
    }

    #[cfg(test)]
    pub async fn store_blobs(&self, blobs: Vec<crate::BlobUpload>) -> Result<()> {
        for blob in blobs {
            self.backend.upload_stream(blob.into()).await?;
        }
        Ok(())
    }

    // Store file into the backend at the given path, uncompressed.
    // The path will also be used to determine the mime type.
    #[instrument(skip(self, content))]
    pub async fn store_one_uncompressed(
        &self,
        path: impl Into<String> + fmt::Debug,
        content: impl Into<Bytes>,
    ) -> Result<()> {
        let path = path.into();
        let content = content.into();
        let mime = detect_mime(&path).to_owned();

        self.backend
            .upload_stream(StreamUpload {
                path,
                mime,
                source: StreamUploadSource::Bytes(content),
                compression: None,
            })
            .await?;

        Ok(())
    }

    // Store file into the backend at the given path (also used to detect mime type), returns the
    // chosen compression algorithm
    #[instrument(skip(self, content))]
    pub async fn store_one(
        &self,
        path: impl Into<String> + fmt::Debug,
        content: impl Into<Bytes>,
    ) -> Result<CompressionAlgorithm> {
        let path = path.into();
        let alg = CompressionAlgorithm::default();
        let content = compress(Cursor::new(content.into().as_ref()), alg)?;
        let mime = detect_mime(&path).to_owned();

        self.backend
            .upload_stream(StreamUpload {
                path,
                mime,
                source: StreamUploadSource::Bytes(content.into()),
                compression: Some(alg),
            })
            .await?;

        Ok(alg)
    }

    #[instrument(skip(self))]
    pub async fn list_prefix<'a>(&'a self, prefix: &'a str) -> BoxStream<'a, Result<String>> {
        self.backend.list_prefix(prefix).await
    }

    #[instrument(skip(self))]
    pub async fn delete_prefix(&self, prefix: &str) -> Result<()> {
        self.backend.delete_prefix(prefix).await
    }

    // We're using `&self` instead of consuming `self` or creating a Drop impl because during tests
    // we leak the web server, and Drop isn't executed in that case (since the leaked web server
    // still holds a reference to the storage).
    #[cfg(any(test, feature = "testing"))]
    pub(crate) async fn cleanup_after_test(&self) -> Result<()> {
        if let StorageBackend::S3(s3) = &self.backend {
            s3.cleanup_after_test().await?;
        }
        Ok(())
    }
}

impl archive_index::Downloader for AsyncStorage {
    fn fetch_archive_index<'a>(
        &'a self,
        remote_index_path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<StreamingBlob>> + Send + 'a>> {
        Box::pin(self.get_stream(remote_index_path))
    }
}

impl fmt::Debug for AsyncStorage {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match &self.backend {
            #[cfg(any(test, feature = "testing"))]
            StorageBackend::Memory(_) => write!(f, "memory-backed storage"),
            StorageBackend::S3(_) => write!(f, "S3-backed storage"),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::testing::TestStorage;
    use tokio::fs;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_outdated_local_archive_index_gets_redownloaded() -> Result<()> {
        let metrics = docs_rs_opentelemetry::testing::TestMetrics::new();
        let storage = TestStorage::from_kind(StorageKind::S3, metrics.provider()).await?;

        // virtual latest build id, used for local caching of the index files
        const LATEST_BUILD_ID: Option<BuildId> = Some(BuildId(42));
        let cache_root = storage.config.archive_index_cache.path.clone();

        let cache_filename = |archive_name: &str| {
            cache_root.join(format!(
                "{}.{}.{}",
                archive_name,
                LATEST_BUILD_ID.unwrap(),
                ARCHIVE_INDEX_FILE_EXTENSION
            ))
        };

        const IMPORTANT_NAME: &str = "important.txt";
        const IMPORTANT_CONTENT: &[u8] = b"important.txt";

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
                let content = if file == IMPORTANT_NAME {
                    IMPORTANT_CONTENT
                } else {
                    archive_name.as_bytes()
                };
                fs::write(path, content).await?;
            }
            storage
                .store_all_in_archive(archive_name, dir.path())
                .await?;

            Ok(())
        }

        const ARCHIVE_1: &str = "test_short.zip";
        const ARCHIVE_2: &str = "test_long_name.zip";
        assert!(ARCHIVE_1.len() != ARCHIVE_2.len());
        const ARCHIVES: [&str; 2] = [ARCHIVE_1, ARCHIVE_2];

        // create two archives with indexes that contain the same filename
        // The content of the files in each archive need to differ even with the same name,
        // so we can assert the different ranges below.
        create_archive(
            &storage,
            ARCHIVE_1,
            &["file1.txt", "file2.txt", IMPORTANT_NAME],
        )
        .await?;

        create_archive(
            &storage,
            ARCHIVE_2,
            &[IMPORTANT_NAME, "another_file_1.txt", "another_file_2.txt"],
        )
        .await?;

        for archive_name in ARCHIVES {
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
                    .exists_in_archive(archive_name, LATEST_BUILD_ID, IMPORTANT_NAME)
                    .await?
            );
            assert!(fs::try_exists(&local_index_file).await?);

            // fetching the content out of the archive also works
            assert_eq!(
                storage
                    .get_from_archive(archive_name, LATEST_BUILD_ID, IMPORTANT_NAME)
                    .await?
                    .content,
                archive_name.as_bytes()
            );
        }

        // validate if the positions are really different in the archvies,
        // for the same filename.
        let pos_in_test1_zip = storage
            .find_archive_index(ARCHIVE_1, LATEST_BUILD_ID)
            .await?
            .find(IMPORTANT_NAME)
            .await?
            .unwrap();
        let pos_in_test2_zip = storage
            .find_archive_index(ARCHIVE_2, LATEST_BUILD_ID)
            .await?
            .find(IMPORTANT_NAME)
            .await?
            .unwrap();

        assert_ne!(pos_in_test1_zip.range(), pos_in_test2_zip.range());

        // now I'm swapping the local index files.
        // This should simulate hat I have an outdated byte-range for a file

        let local_index_file_1 = cache_filename(ARCHIVE_1);
        let local_index_file_2 = cache_filename(ARCHIVE_2);

        {
            let temp_path = cache_root.join("temp_index_swap.tmp");
            fs::rename(&local_index_file_1, &temp_path).await?;
            fs::rename(&local_index_file_2, &local_index_file_1).await?;
            fs::rename(&temp_path, &local_index_file_2).await?;
        }

        // now try to fetch the files inside the archives again, the local files
        // should be removed, refetched, and all should be fine.
        // Without our fallback / delete mechanism, this would fail.

        for archive_name in ARCHIVES {
            assert_eq!(
                storage
                    .get_from_archive(archive_name, LATEST_BUILD_ID, IMPORTANT_NAME)
                    .await?
                    .content,
                archive_name.as_bytes(),
            );
        }

        Ok(())
    }
}

/// Backend tests are a set of tests executed on all the supportedootorage backends. They ensure
/// docs.rs behaves the same no matter the storage backend currently used.
///
/// To add a new test create the function without adding the `#[test]` attribute, and add the
/// function name to the `backend_tests!` macro at the bottom of the module.
///
/// This is the preferred way to test whether backends work.
#[cfg(test)]
mod backend_tests {
    use super::*;
    use crate::blob::BlobUpload;
    use crate::{PathNotFoundError, errors::SizeLimitReached};
    use docs_rs_headers::compute_etag;
    use docs_rs_opentelemetry::testing::TestMetrics;

    fn get_file_info(files: &[FileEntry], path: impl AsRef<Path>) -> Option<&FileEntry> {
        let path = path.as_ref();
        files.iter().find(|info| info.path == path)
    }

    async fn test_exists(storage: &AsyncStorage) -> Result<()> {
        assert!(!storage.exists("path/to/file.txt").await.unwrap());
        let blob = BlobUpload {
            path: "path/to/file.txt".into(),
            mime: mime::TEXT_PLAIN,
            content: "Hello world!".into(),
            compression: None,
        };
        storage.store_blobs(vec![blob]).await?;
        assert!(storage.exists("path/to/file.txt").await?);

        Ok(())
    }

    async fn test_get_object(storage: &AsyncStorage) -> Result<()> {
        let path: &str = "foo/bar.txt";
        let blob = BlobUpload {
            path: path.into(),
            mime: mime::TEXT_PLAIN,
            compression: None,
            content: "test content\n".into(),
        };

        storage.store_blobs(vec![blob.clone()]).await?;

        let found = storage.get(path, usize::MAX).await?;
        assert_eq!(blob.mime, found.mime);
        assert_eq!(blob.content, found.content);
        // while our db backend just does MD5,
        // it seems like minio does it too :)
        assert_eq!(found.etag, Some(compute_etag(&blob.content)));

        for path in &["bar.txt", "baz.txt", "foo/baz.txt"] {
            assert!(
                storage
                    .get(path, usize::MAX)
                    .await
                    .unwrap_err()
                    .downcast_ref::<PathNotFoundError>()
                    .is_some()
            );
        }

        Ok(())
    }

    async fn test_get_range(storage: &AsyncStorage) -> Result<()> {
        let blob = BlobUpload {
            path: "foo/bar.txt".into(),
            mime: mime::TEXT_PLAIN,
            compression: None,
            content: "test content\n".into(),
        };

        let full_etag = compute_etag(&blob.content);

        storage.store_blobs(vec![blob.clone()]).await?;

        let mut etags = Vec::new();

        for range in [0..=4, 5..=12] {
            let partial_blob = storage
                .get_range_stream("foo/bar.txt", range.clone(), None)
                .await?
                .materialize(usize::MAX)
                .await?;
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
                    .get_range_stream(path, 0..=4, None)
                    .await
                    .unwrap_err()
                    .downcast_ref::<PathNotFoundError>()
                    .is_some()
            );
        }

        Ok(())
    }

    async fn test_list_prefix(storage: &AsyncStorage) -> Result<()> {
        static FILENAMES: &[&str] = &["baz.txt", "some/bar.txt"];

        storage
            .store_blobs(
                FILENAMES
                    .iter()
                    .map(|&filename| BlobUpload {
                        path: filename.into(),
                        mime: mime::TEXT_PLAIN,
                        compression: None,
                        content: "test content\n".into(),
                    })
                    .collect(),
            )
            .await?;

        assert_eq!(
            storage
                .list_prefix("")
                .await
                .try_collect::<Vec<_>>()
                .await?,
            FILENAMES
        );

        assert_eq!(
            storage
                .list_prefix("some/")
                .await
                .try_collect::<Vec<_>>()
                .await?,
            &["some/bar.txt"]
        );

        Ok(())
    }

    async fn test_too_long_filename(storage: &AsyncStorage) -> Result<()> {
        // minio returns ErrKeyTooLongError when the key is over 1024 bytes long.
        // When testing, minio just gave me `XMinioInvalidObjectName`, so I'll check that too.
        let long_filename = "ATCG".repeat(512);

        assert!(
            storage
                .get(&long_filename, 42)
                .await
                .unwrap_err()
                .is::<PathNotFoundError>()
        );

        Ok(())
    }

    async fn test_get_too_big(storage: &AsyncStorage) -> Result<()> {
        const MAX_SIZE: usize = 1024;

        let small_blob = BlobUpload {
            path: "small-blob.bin".into(),
            mime: mime::TEXT_PLAIN,
            content: [0; MAX_SIZE].as_ref().into(),
            compression: None,
        };
        let big_blob = BlobUpload {
            path: "big-blob.bin".into(),
            mime: mime::TEXT_PLAIN,
            content: [0; MAX_SIZE * 2].as_ref().into(),
            compression: None,
        };

        storage
            .store_blobs(vec![small_blob.clone(), big_blob])
            .await?;

        let blob = storage.get("small-blob.bin", MAX_SIZE).await?;
        assert_eq!(blob.content.len(), small_blob.content.len());

        assert!(
            storage
                .get("big-blob.bin", MAX_SIZE)
                .await
                .unwrap_err()
                .downcast_ref::<std::io::Error>()
                .and_then(|io| io.get_ref())
                .and_then(|err| err.downcast_ref::<SizeLimitReached>())
                .is_some()
        );

        Ok(())
    }

    async fn test_store_blobs(storage: &AsyncStorage, metrics: &TestMetrics) -> Result<()> {
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
                content: "Hello world!\n".into(),
            })
            .collect::<Vec<_>>();

        storage.store_blobs(blobs.clone()).await.unwrap();

        for blob in &blobs {
            let actual = storage.get(&blob.path, usize::MAX).await?;
            assert_eq!(blob.path, actual.path);
            assert_eq!(blob.mime, actual.mime);
        }

        let collected_metrics = metrics.collected_metrics();

        assert_eq!(
            collected_metrics
                .get_metric("storage", "docsrs.storage.uploaded_files")?
                .get_u64_counter()
                .value(),
            NAMES.len() as u64,
        );

        Ok(())
    }

    async fn test_exists_without_remote_archive(storage: &AsyncStorage) -> Result<()> {
        // when remote and local index don't exist, any `exists_in_archive`  should
        // return `false`
        assert!(
            !storage
                .exists_in_archive("some_archive_name", None, "some_file_name")
                .await?
        );
        Ok(())
    }

    async fn test_store_all_in_archive(
        storage: &AsyncStorage,
        metrics: &TestMetrics,
    ) -> Result<()> {
        let dir = tempfile::Builder::new()
            .prefix("docs.rs-upload-archive-test")
            .tempdir()?;
        let files = ["Cargo.toml", "src/main.rs"];
        for &file in &files {
            let path = dir.path().join(file);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::write(path, "data").await?;
        }

        let local_index_location = storage
            .config
            .archive_index_cache
            .path
            .join(format!("folder/test.zip.0.{ARCHIVE_INDEX_FILE_EXTENSION}"));

        let (stored_files, compression_alg) = storage
            .store_all_in_archive("folder/test.zip", dir.path())
            .await?;

        assert!(
            storage
                .exists(&format!("folder/test.zip.{ARCHIVE_INDEX_FILE_EXTENSION}"))
                .await?
        );

        assert_eq!(compression_alg, CompressionAlgorithm::Deflate);
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
            fs::remove_file(&local_index_location).await?;
        }

        // the first exists-query will download and store the index
        assert!(!local_index_location.exists());
        assert!(
            storage
                .exists_in_archive("folder/test.zip", None, "Cargo.toml")
                .await?
        );

        // the second one will use the local index
        assert!(local_index_location.exists());
        assert!(
            storage
                .exists_in_archive("folder/test.zip", None, "src/main.rs")
                .await?
        );

        let file = storage
            .get_from_archive("folder/test.zip", None, "Cargo.toml")
            .await?;
        assert_eq!(file.content, b"data");
        assert_eq!(file.mime, "text/toml");
        assert_eq!(file.path, "folder/test.zip/Cargo.toml");

        let file = storage
            .get_from_archive("folder/test.zip", None, "src/main.rs")
            .await?;
        assert_eq!(file.content, b"data");
        assert_eq!(file.mime, "text/rust");
        assert_eq!(file.path, "folder/test.zip/src/main.rs");

        let collected_metrics = metrics.collected_metrics();

        assert_eq!(
            collected_metrics
                .get_metric("storage", "docsrs.storage.uploaded_files")?
                .get_u64_counter()
                .value(),
            2,
        );

        Ok(())
    }

    async fn test_store_all(storage: &AsyncStorage, metrics: &TestMetrics) -> Result<()> {
        let dir = tempfile::Builder::new()
            .prefix("docs.rs-upload-test")
            .tempdir()?;
        let files = ["Cargo.toml", "src/main.rs"];
        for &file in &files {
            let path = dir.path().join(file);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).await?;
            }
            fs::write(path, "data").await?;
        }

        let (stored_files, algs) = storage.store_all(Path::new("prefix"), dir.path()).await?;
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

        let file = storage.get("prefix/Cargo.toml", usize::MAX).await?;
        assert_eq!(file.content, b"data");
        assert_eq!(file.mime, "text/toml");
        assert_eq!(file.path, "prefix/Cargo.toml");

        let file = storage.get("prefix/src/main.rs", usize::MAX).await?;
        assert_eq!(file.content, b"data");
        assert_eq!(file.mime, "text/rust");
        assert_eq!(file.path, "prefix/src/main.rs");

        assert_eq!(algs, CompressionAlgorithm::default());

        let collected_metrics = metrics.collected_metrics();
        assert_eq!(
            collected_metrics
                .get_metric("storage", "docsrs.storage.uploaded_files")?
                .get_u64_counter()
                .value(),
            2,
        );

        Ok(())
    }

    async fn test_batched_uploads(storage: &AsyncStorage) -> Result<()> {
        let uploads: Vec<_> = (0..=100)
            .map(|i| BlobUpload {
                mime: mimes::TEXT_RUST.clone(),
                content: format!("const IDX: usize = {i};").into(),
                path: format!("{i}.rs"),
                compression: None,
            })
            .collect();

        storage.store_blobs(uploads.clone()).await?;

        for blob in &uploads {
            let stored = storage.get(&blob.path, usize::MAX).await?;
            assert_eq!(&stored.content, &blob.content);
        }

        Ok(())
    }

    async fn test_s3_large_file_upload_uses_multipart(storage: &AsyncStorage) -> Result<()> {
        if matches!(storage.config.storage_backend, StorageKind::Memory) {
            return Ok(());
        }

        const MULTIPART_UPLOAD_SIZE: usize = 100 * 1024 * 1024 + 10;
        const REMOTE_PATH: &str = "multipart-upload-test.bin";

        let dir = tempfile::Builder::new()
            .prefix("docs.rs-multipart-upload-test")
            .tempdir()?;
        let local_path = dir.path().join("large.bin");
        let content = (0..MULTIPART_UPLOAD_SIZE)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<_>>();
        fs::write(&local_path, &content).await?;

        storage
            .backend
            .upload_stream(StreamUpload {
                path: REMOTE_PATH.into(),
                mime: mime::APPLICATION_OCTET_STREAM,
                source: StreamUploadSource::File(local_path),
                compression: None,
            })
            .await?;

        assert!(storage.exists(REMOTE_PATH).await?);
        let stored = storage.get(REMOTE_PATH, usize::MAX).await?;
        assert_eq!(stored.content, content);
        storage.delete_prefix(REMOTE_PATH).await?;

        Ok(())
    }

    async fn test_delete_prefix_without_matches(storage: &AsyncStorage) -> Result<()> {
        storage.delete_prefix("prefix_without_objects").await
    }

    async fn test_delete_prefix(storage: &AsyncStorage) -> Result<()> {
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
        .await
    }

    async fn test_delete_percent(storage: &AsyncStorage) -> Result<()> {
        // PostgreSQL treats "%" as a special char when deleting a prefix. Make sure any "%" in the
        // provided prefix is properly escaped.
        test_deletion(
            storage,
            "foo/%/",
            &["foo/bar.txt", "foo/%/bar.txt"],
            &["foo/bar.txt"],
            &["foo/%/bar.txt"],
        )
        .await
    }

    async fn test_deletion(
        storage: &AsyncStorage,
        prefix: &str,
        start: &[&str],
        present: &[&str],
        missing: &[&str],
    ) -> Result<()> {
        storage
            .store_blobs(
                start
                    .iter()
                    .map(|path| BlobUpload {
                        path: (*path).to_string(),
                        content: "foo\n".into(),
                        compression: None,
                        mime: mime::TEXT_PLAIN,
                    })
                    .collect(),
            )
            .await?;

        storage.delete_prefix(prefix).await?;

        for existing in present {
            assert!(storage.get(existing, usize::MAX).await.is_ok());
        }
        for missing in missing {
            assert!(
                storage
                    .get(missing, usize::MAX)
                    .await
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
                    use crate::types::StorageKind;
                    use crate::testing::TestStorage;
                    use docs_rs_opentelemetry::testing::TestMetrics;

                    async fn get_storage() -> anyhow::Result<(TestStorage, TestMetrics)> {
                        let metrics = TestMetrics::new();
                        let storage = TestStorage::from_kind($config, metrics.provider()).await?;
                        Ok((storage, metrics))
                    }


                    backend_tests!(@tests $tests);
                    backend_tests!(@tests_with_metrics $tests_with_metrics);
                }
            )*
        };
        (@tests { $($test:ident,)* }) => {
            $(
                #[tokio::test(flavor = "multi_thread")]
                async fn $test() -> anyhow::Result<()> {
                    let (storage, _metrics) = get_storage().await?;
                    super::$test(&storage).await
                }
            )*
        };
        (@tests_with_metrics { $($test:ident,)* }) => {
            $(
                #[tokio::test(flavor = "multi_thread")]
                async fn $test() -> anyhow::Result<()> {
                    let (storage, metrics) = get_storage().await?;
                    super::$test(&storage, &metrics).await
                }
            )*
        };
    }

    backend_tests! {
        backends {
            s3 => StorageKind::S3,
            memory => StorageKind::Memory,
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
            test_s3_large_file_upload_uses_multipart,
        }

        tests_with_metrics {
            test_store_blobs,
            test_store_all,
            test_store_all_in_archive,
        }
    }
}
