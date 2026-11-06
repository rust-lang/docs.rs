use crate::{
    PathNotFoundError, blob::StreamingBlob, config::ArchiveIndexCacheConfig, file::FolderEntry,
    types::FileRange, utils::file_list::walk_dir_recursive,
};
use anyhow::{Context as _, Result, anyhow, bail};
use async_stream::try_stream;
use docs_rs_opentelemetry::AnyMeterProvider;
use docs_rs_types::{BuildId, ByteSize, CompressionAlgorithm};
use docs_rs_utils::spawn_blocking;
use futures_util::{Stream, TryStreamExt as _};
use moka::{future::Cache as MokaCache, notification::RemovalCause};
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Gauge, Histogram},
};
use rusqlite::{Connection, OpenFlags, OptionalExtension as _, params};
use std::{
    fmt,
    future::Future,
    mem,
    path::{self, Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    fs,
    io::{self, AsyncRead, AsyncSeek, AsyncWriteExt as _},
    sync::{Mutex, mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::io::SyncIoBridge;
use tracing::{debug, error, info, info_span, instrument, trace, warn};
use url::Url;

pub(crate) const ARCHIVE_INDEX_FILE_EXTENSION: &str = "index";

/// dummy size we assume in case of errors
const DUMMY_FILE_SIZE: ByteSize = ByteSize::mib(1);
/// self-repair attempts
const REPAIR_ATTEMPTS: usize = 5;

/// Size buckets for downloaded and evicted archive-index files, doubling from 512 KiB to 16 GiB.
const ARCHIVE_INDEX_SIZE_BUCKETS: &[ByteSize; 16] = &{
    let mut buckets = [ByteSize::kib(512); 16];
    let mut i = 1;
    while i < buckets.len() {
        buckets[i] = ByteSize::b(buckets[i - 1].as_u64() * 2);
        i += 1;
    }
    buckets
};

#[derive(Debug)]
struct Metrics {
    // calls to find an entry in the local cache
    find_calls: Counter<u64>,

    // local cache eviction
    evicted_entries: Counter<u64>,
    evicted_bytes_total: Counter<u64>,
    evicted_entry_size: Histogram<u64>,

    // local cache misses / downloads & bytes
    // includes retries / repairs
    downloads: Counter<u64>,
    downloaded_bytes: Counter<u64>,
    downloaded_entry_size: Histogram<u64>,

    // full cache size (count / bytes)
    weighted_size_bytes: Gauge<u64>,
    entry_count: Gauge<u64>,
}

impl Metrics {
    fn new(meter_provider: &AnyMeterProvider) -> Self {
        let meter = meter_provider.meter("storage");
        const PREFIX: &str = "docsrs.storage.archive_index_cache";

        let entry_size_boundaries: Vec<f64> = ARCHIVE_INDEX_SIZE_BUCKETS
            .iter()
            .map(|size| size.as_u64() as f64)
            .collect();

        Self {
            find_calls: meter
                .u64_counter(format!("{PREFIX}.find_total"))
                .with_unit("1")
                .build(),
            downloads: meter
                .u64_counter(format!("{PREFIX}.download_total"))
                .with_unit("1")
                .build(),
            downloaded_bytes: meter
                .u64_counter(format!("{PREFIX}.download_bytes_total"))
                .with_unit("By")
                .build(),
            evicted_entries: meter
                .u64_counter(format!("{PREFIX}.eviction_total"))
                .with_unit("1")
                .build(),
            evicted_bytes_total: meter
                .u64_counter(format!("{PREFIX}.evicted_bytes_total"))
                .with_unit("By")
                .build(),
            evicted_entry_size: meter
                .u64_histogram(format!("{PREFIX}.evicted_entry_size"))
                .with_unit("By")
                .with_boundaries(entry_size_boundaries.clone())
                .build(),
            downloaded_entry_size: meter
                .u64_histogram(format!("{PREFIX}.downloaded_entry_size"))
                .with_unit("By")
                .with_boundaries(entry_size_boundaries)
                .build(),
            weighted_size_bytes: meter
                .u64_gauge(format!("{PREFIX}.weighted_size_bytes"))
                .with_unit("By")
                .build(),
            entry_count: meter
                .u64_gauge(format!("{PREFIX}.entry_count"))
                .with_unit("1")
                .build(),
        }
    }

    fn record_download(&self, size: ByteSize) {
        let size = size.as_u64();
        self.downloads.add(1, &[]);
        self.downloaded_bytes.add(size, &[]);
        self.downloaded_entry_size.record(size, &[]);
    }

    fn record_evicted_entry(&self, entry: &Entry, reason: RemovalCause) {
        let reason = format!("{reason:?}");
        let evicted_bytes = entry.file_size;
        let reason_attr = [KeyValue::new("cause", reason)];

        self.evicted_entries.add(1, &reason_attr);
        self.evicted_bytes_total
            .add(evicted_bytes.as_u64(), &reason_attr);
        self.evicted_entry_size
            .record(evicted_bytes.as_u64(), &reason_attr);
    }
}

#[derive(PartialEq, Eq, Debug)]
pub struct FileInfo {
    path: PathBuf,
    range: FileRange,
    compression: CompressionAlgorithm,
}

pub(crate) struct Entry {
    // file size of the local sqlite database.
    file_size: ByteSize,
}

impl Entry {
    // Will be used to "weigh" cache entries, so that the cache can evict based on
    // total size of cached files instead of number of entries.
    fn weight(&self) -> u32 {
        self.file_size.as_kb() as u32
    }

    fn from_size(file_size: ByteSize) -> Self {
        Self { file_size }
    }

    async fn from_path(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        Self::from_size(match fs::metadata(&path).await {
            Ok(meta) => ByteSize::b(meta.len()),
            Err(err) => {
                warn!(
                    ?err,
                    ?path,
                    "failed to get metadata for local archive index file, using dummy size for cache eviction"
                );
                DUMMY_FILE_SIZE
            }
        })
    }
}

type CacheManager = MokaCache<PathBuf, Arc<Entry>>;
type SqliteThreadPool = Arc<rayon_core::ThreadPool>;

pub fn new_threadpool(num_threads: usize) -> Result<SqliteThreadPool> {
    anyhow::ensure!(
        num_threads > 0,
        "archive index SQLite thread count must be greater than zero"
    );
    Ok(Arc::new(
        rayon_core::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .thread_name(|idx| format!("docsrs-sqlite {idx}"))
            .panic_handler(|panic| {
                let message = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("non-string panic payload");
                error!(message, "archive index SQLite worker panicked");
            })
            .build()?,
    ))
}

/// Local archive index cache.
///
/// Note: "last access" times for cache entries reset on each server startup
/// (the moka cache starts empty and gets backfilled from disk without
/// preserving prior access timestamps). This means TTI-based eviction is
/// uninformed until real traffic re-establishes usage patterns.
///
/// This is acceptable because:
/// - Builds happen infrequently (every couple of months), so cached index
///   data stays valid for a long time. Serving it for an extra TTL window
///   after a restart is harmless.
/// - moka's TinyLFU-based eviction policy adapts quickly once traffic
///   resumes.
/// - Persisting access timestamps would add significant complexity
///   (moka doesn't support injecting custom timestamps on insert) for
///   marginal benefit.
pub(crate) struct Cache {
    config: Arc<ArchiveIndexCacheConfig>,
    /// Tracks locally cached archive indices and coordinates their initialization & invalidation.
    manager: CacheManager,
    /// Dedicated workers shared by all archive-index readers in this cache.
    threadpool: SqliteThreadPool,
    metrics: Arc<Metrics>,
    background_tasks: Vec<JoinHandle<()>>,
}

pub(crate) trait Downloader {
    fn fetch_archive_index<'a>(
        &'a self,
        remote_index_path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<StreamingBlob>> + Send + 'a>>;
}

impl Cache {
    /// create a new archive index cache.
    ///
    /// Also starts a background task that will backfill the in-memory cache management based
    /// on the local files that are already present.
    pub(crate) async fn new(
        config: Arc<ArchiveIndexCacheConfig>,
        meter_provider: &AnyMeterProvider,
    ) -> Result<Self> {
        let mut cache = Self::new_inner(config.clone(), meter_provider).await?;

        cache.background_tasks.push(tokio::spawn({
            let manager = cache.manager.clone();
            async move {
                if let Err(err) = Self::backfill_cache_manager(config, manager).await {
                    error!(?err, "failed to backfill archive index cache manager");
                }
            }
        }));

        Ok(cache)
    }

    /// create a new archive index cache, and directly backfill the in-memory structures.
    ///
    /// Only for testing.
    #[cfg(test)]
    async fn new_with_backfill(
        config: Arc<ArchiveIndexCacheConfig>,
        meter_provider: &AnyMeterProvider,
    ) -> Result<Self> {
        let cache = Self::new_inner(config.clone(), meter_provider).await?;

        Self::backfill_cache_manager(config, cache.manager.clone())
            .await
            .context("failed to backfill archive index cache manager")?;

        Ok(cache)
    }

    async fn new_inner(
        config: Arc<ArchiveIndexCacheConfig>,
        meter_provider: &AnyMeterProvider,
    ) -> Result<Self> {
        fs::create_dir_all(&config.path)
            .await
            .context("failed to create archive index cache directory")?;

        let threadpool = new_threadpool(config.sqlite_threads.get())?;

        let metrics = Arc::new(Metrics::new(meter_provider));
        let metrics_for_eviction = metrics.clone();
        let manager = CacheManager::builder()
            .initial_capacity(config.expected_count)
            // configured eviction policy
            .eviction_policy(config.eviction_policy.into())
            // Time to idle (TTI): A cached entry will be expired after
            // the specified duration past from get or insert.
            // We don't set TTL (time to live), which would be just time-after-insert.
            .time_to_idle(config.ttl.into())
            // We weigh each cache entry by the file size of the SQLite database.
            // The configured capacity is in MiB, but using KiB as moka's weight unit
            // avoids counting every index smaller than 1 MiB as if it were 1 MiB.
            .weigher(|_key: &PathBuf, entry: &Arc<Entry>| -> u32 { entry.weight() })
            // max capacity
            // not entries, but _weighted entries_.
            // with the weight fn from above, the max capacity is a storage size value.
            .max_capacity(config.max_size.as_kib() as u64)
            // the eviction listener is called when moka evicts a cache entry.
            // In this case we want to delete the corresponding local files.
            .async_eviction_listener(move |path, entry, reason| {
                let path = path.to_path_buf();
                let metrics = metrics_for_eviction.clone();
                Box::pin(async move {
                    metrics.record_evicted_entry(&entry, reason);

                    // Explicit invalidation leaves file cleanup to the caller
                    // (`purge` or the repair initializer in `find_index_inner`).
                    // Deleting again could remove a newly downloaded replacement.
                    if reason == RemovalCause::Explicit {
                        return;
                    }

                    trace!(
                        ?path,
                        ?reason,
                        "evicting local archive index file from cache"
                    );
                    if let Err(err) = Self::remove_local_index(&path).await {
                        error!(
                            ?err,
                            ?path,
                            ?reason,
                            "failed to remove local archive index file on cache eviction"
                        );
                    }
                })
            })
            .build();

        let handle = tokio::spawn({
            let manager = manager.clone();
            let metrics = metrics.clone();

            // moka will also run maintenance tasks itself, but I want to force this
            // at least every 30 seconds.
            //
            // We also use this background task to gather metrics.
            async move {
                let mut interval = tokio::time::interval(Duration::from_secs(30));
                loop {
                    interval.tick().await;

                    debug!("running pending tasks for archive index cache manager");
                    manager.run_pending_tasks().await;

                    debug!("collect cache size metrics");
                    metrics.entry_count.record(manager.entry_count(), &[]);
                    metrics
                        .weighted_size_bytes
                        .record(manager.weighted_size() * 1024, &[]);
                }
            }
        });

        let cache = Self {
            threadpool,
            manager,
            config,
            metrics,
            background_tasks: vec![handle],
        };
        Ok(cache)
    }

    /// run any pending tasks, like evictions that need to delete local files.
    #[cfg(test)]
    async fn flush(&self) -> Result<()> {
        self.manager.run_pending_tasks().await;
        Ok(())
    }

    #[cfg(test)]
    async fn backfill(&self) -> Result<()> {
        Self::backfill_cache_manager(self.config.clone(), self.manager.clone()).await
    }

    /// backfill the in memory cache management based on the local files that are already
    /// present on disk.
    ///
    /// Should be needed only once after server startup.
    ///
    /// While this is running, `find_index_inner` and `download_archive_index` backfill
    /// entries on demand for requested indexes.
    ///
    /// Concurrency is set to a lower value intentionally so we don't put
    /// too much i/o pressure onto the disk.
    #[instrument(skip_all)]
    async fn backfill_cache_manager(
        config: Arc<ArchiveIndexCacheConfig>,
        manager: CacheManager,
    ) -> Result<()> {
        info!(path=%config.path.display(), "starting cache-manager backfill from local directory");
        let inserted = Arc::new(AtomicU64::new(0));

        walk_dir_recursive(&config.path)
            .err_into::<anyhow::Error>()
            .try_for_each_concurrent(Some(4), |item| {
                let manager = manager.clone();
                let inserted = inserted.clone();
                async move {
                    let path = item.absolute;
                    if path.extension().and_then(|ext| ext.to_str())
                        == Some(ARCHIVE_INDEX_FILE_EXTENSION)
                    {
                        let entry = manager
                            .entry(path)
                            .or_insert_with(async {
                                Arc::new(Entry::from_size(item.metadata.len().into()))
                            })
                            .await;

                        if entry.is_fresh() {
                            inserted.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Ok(())
                }
            })
            .await?;

        info!(
            inserted_count = inserted.load(Ordering::Relaxed),
            "finished cache-manager backfill"
        );
        Ok(())
    }

    async fn remove_local_index(index_path: impl AsRef<Path>) -> Result<()> {
        let index_path = index_path.as_ref();
        let wal = index_path.with_extension(format!("{ARCHIVE_INDEX_FILE_EXTENSION}-wal"));
        let shm = index_path.with_extension(format!("{ARCHIVE_INDEX_FILE_EXTENSION}-shm"));

        let (_, _, index_result) = tokio::join!(
            fs::remove_file(wal),
            fs::remove_file(shm),
            fs::remove_file(index_path),
        );

        if let Err(err) = index_result
            && err.kind() != io::ErrorKind::NotFound
        {
            Err(err.into())
        } else {
            Ok(())
        }
    }

    fn local_index_path(&self, archive_path: &str, latest_build_id: Option<BuildId>) -> PathBuf {
        self.config.path.join(format!(
            "{archive_path}.{}.{ARCHIVE_INDEX_FILE_EXTENSION}",
            latest_build_id.map(|id| id.0).unwrap_or(0)
        ))
    }

    /// purge a single archive index file
    pub(crate) async fn purge(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
    ) -> Result<()> {
        let local_index_path = self.local_index_path(archive_path, latest_build_id);
        Self::remove_local_index(&local_index_path).await?;
        self.manager.invalidate(&local_index_path).await;

        Ok(())
    }

    async fn retry_with_purge<T, F, Fut>(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        mut action: F,
    ) -> Result<(T, usize)>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        for attempt in 1..=REPAIR_ATTEMPTS {
            let err = match action().await {
                Ok(value) => return Ok((value, attempt)),
                Err(err) => err,
            };

            // A missing remote index cannot be repaired by purging the local cache.
            if err.is::<PathNotFoundError>() || attempt == REPAIR_ATTEMPTS {
                return Err(err);
            }

            // Retry once before purging, or on CannotOpen errors: another request
            // may have published a replacement since the failure.
            if attempt == 1 || cannot_open_index(&err) {
                debug!(?err, %attempt, "retrying archive index operation without purging");
            } else {
                warn!(
                    ?err,
                    %attempt,
                    "archive index operation failed, purging local cache and retrying"
                );
                self.purge(archive_path, latest_build_id).await?;
            }
        }

        unreachable!("archive index retry loop exited unexpectedly");
    }

    #[instrument(skip(self, downloader))]
    pub(crate) async fn find_index<D: Downloader + Sync>(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        downloader: &D,
    ) -> Result<Index> {
        let (index, _) = self
            .retry_with_purge(archive_path, latest_build_id, || {
                self.find_index_inner(archive_path, latest_build_id, downloader)
            })
            .await?;
        Ok(index)
    }

    #[instrument(skip(self, downloader))]
    async fn find_index_inner<D: Downloader + Sync>(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        downloader: &D,
    ) -> Result<Index> {
        let local_index_path = self.local_index_path(archive_path, latest_build_id);

        // fast path: try to use whatever is there, no locking
        match Index::open(&local_index_path, self.threadpool.clone()).await {
            Ok(index) => {
                // Keep moka's recency/frequency view in sync with successful fast-path
                // file lookups so TTI and admission decisions reflect real usage.
                if self.manager.get(&local_index_path).await.is_none() {
                    let entry_path = local_index_path.clone();
                    self.manager
                        .entry(local_index_path.clone())
                        .or_insert_with(
                            async move { Arc::new(Entry::from_path(&entry_path).await) },
                        )
                        .await;
                }

                return Ok(index);
            }
            Err(err) => {
                debug!(?err, "archive index open failed, will try repair.");
            }
        }

        let remote_index_path = format!("{archive_path}.{ARCHIVE_INDEX_FILE_EXTENSION}");
        self.repair_index(&local_index_path, &remote_index_path, downloader)
            .await
    }

    #[instrument(skip(self, downloader))]
    async fn repair_index<D: Downloader + Sync>(
        &self,
        local_index_path: &Path,
        remote_index_path: &str,
        downloader: &D,
    ) -> Result<Index> {
        // The file is missing or could not be opened, but Moka may still have an
        // entry for it. That entry would make `try_get_with_by_ref` skip the repair
        // initializer below, leaving the final open to fail again and forcing the
        // outer loop to purge and retry. Invalidate it so this attempt can repair
        // the file immediately. Successful fast-path opens return before this.
        //
        // Explicit invalidation does not delete files; the eviction listener skips
        // that cleanup so it cannot later delete a newly downloaded replacement.
        // The coalesced initializer below handles cleanup and download instead.
        //
        // The failed open and this invalidation are not atomic: another request
        // may repair the file between them, and we may invalidate its fresh entry.
        // Recheck the file inside the initializer before deciding to replace it,
        // so a delayed request can reuse another request's repair. This still does
        // not make the whole open/invalidate/repair sequence atomic.
        self.manager.invalidate(local_index_path).await;

        // moka will coalesce all concurrent calls to try_get_with_by_ref with the same key
        // into a single call to the async closure.
        // https://docs.rs/moka/0.12.14/moka/future/struct.Cache.html#concurrent-calls-on-the-same-key
        // So we don't need any locking here to prevent multiple downloads for the same
        // missing archive index.
        self.manager
            .try_get_with_by_ref(local_index_path, async {
                // The initializer's filesystem operations can still overlap eviction
                // cleanup. The final open validates the file after initialization;
                // the caller's bounded retry loop handles any remaining race.
                let entry = match Index::open(local_index_path, self.threadpool.clone()).await {
                    Ok(_index) => Entry::from_path(local_index_path).await,
                    Err(err) => {
                        // Base cleanup on the current failure, not the original
                        // fast-path error: another request may have repaired the
                        // file while this request was waiting to initialize it.
                        if !err.is::<PathNotFoundError>() && !cannot_open_index(&err) {
                            Self::remove_local_index(local_index_path).await?;
                        }
                        Entry::from_size(
                            self.download_archive_index(
                                downloader,
                                local_index_path,
                                remote_index_path,
                            )
                            .await?,
                        )
                    }
                };
                Ok::<_, anyhow::Error>(Arc::new(entry))
            })
            .await
            .map_err(|arc_err: Arc<anyhow::Error>| {
                // We can't convert this Arc<Error> into the inner error type.
                // See https://github.com/moka-rs/moka/issues/497
                // But since some callers are specifically checking
                // ::is<PathNotFoundError> to differentiate other errors from
                // the "not found" case, we want to preserve that information
                // if it was the cause of the error.
                //
                // This mean all error types that we later want to use with ::is<> or
                // ::downcast<> have to be mentioned here.
                //
                // While we could also migrate to a custom enum error type, this would
                // only be really nice when the whole storage lib uses is. Otherwise
                // we'll end up with some hardcoded conversions again.
                // So I can leave it as-is for now.
                if arc_err.is::<PathNotFoundError>() {
                    anyhow!(PathNotFoundError)
                } else {
                    anyhow!(arc_err)
                }
            })?;

        // Final open for this retry attempt. If it fails, the caller's retry loop
        // purges the cache entry and tries again.
        Index::open(local_index_path, self.threadpool.clone()).await
    }

    /// Find the file metadata needed to fetch a certain path inside a remote archive.
    /// Will try to use a local cache of the index file, and otherwise download it
    /// from storage.
    #[instrument(skip(self, downloader))]
    pub(crate) async fn find<D: Downloader + Sync>(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        path_in_archive: &str,
        downloader: &D,
    ) -> Result<Option<FileInfo>> {
        let result = self
            .retry_with_purge(archive_path, latest_build_id, || async {
                let mut index = self
                    .find_index_inner(archive_path, latest_build_id, downloader)
                    .await?;
                index.find(path_in_archive).await
            })
            .await;

        match result {
            Ok((file_info, attempt)) => {
                self.metrics.find_calls.add(
                    1,
                    &[
                        KeyValue::new("attempt", attempt.to_string()),
                        KeyValue::new("outcome", "success"),
                    ],
                );
                return Ok(file_info);
            }
            Err(err) => {
                self.metrics.find_calls.add(
                    1,
                    &[
                        KeyValue::new("attempt", REPAIR_ATTEMPTS.to_string()),
                        KeyValue::new("outcome", "error"),
                    ],
                );
                return Err(err);
            }
        }
    }

    #[instrument(skip(self, downloader))]
    pub(crate) async fn download_archive_index(
        &self,
        downloader: &impl Downloader,
        local_index_path: &Path,
        remote_index_path: &str,
    ) -> Result<ByteSize> {
        let parent = local_index_path
            .parent()
            .ok_or_else(|| anyhow!("index path without parent"))?
            .to_path_buf();
        fs::create_dir_all(&parent).await?;

        // Create a unique temp file in the cache folder.
        let (temp_file, mut temp_path) = spawn_blocking({
            let folder = self.config.path.clone();
            move || -> Result<_> { tempfile::NamedTempFile::new_in(&folder).map_err(Into::into) }
        })
        .await?
        .into_parts();

        // Download into temp file.
        let mut temp_file = fs::File::from_std(temp_file);
        let mut stream = downloader
            .fetch_archive_index(remote_index_path)
            .await?
            .content;
        let copied = io::copy(&mut stream, &mut temp_file).await?;
        temp_file.flush().await?;

        // Publish atomically.
        // Will replace any existing file.
        fs::rename(&temp_path, local_index_path).await?;

        temp_path.disable_cleanup(true);

        self.metrics.record_download(copied.into());

        Ok(copied.into())
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        for task in &self.background_tasks {
            task.abort();
        }
    }
}

impl FileInfo {
    pub(crate) fn range(&self) -> FileRange {
        self.range.clone()
    }
    pub(crate) fn compression(&self) -> CompressionAlgorithm {
        self.compression
    }
}

const ROW_BATCH_SIZE: usize = 1024;

fn cannot_open_index(err: &anyhow::Error) -> bool {
    err.downcast_ref::<rusqlite::Error>()
        .and_then(rusqlite::Error::sqlite_error_code)
        == Some(rusqlite::ErrorCode::CannotOpen)
}

/// Create a new writable index. Called only from a blocking task.
fn sqlite_create(path: &Path) -> Result<Connection> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "synchronous", "full")?;
    Ok(conn)
}

/// Create an archive index based on a zipfile, replacing any existing index.
#[instrument(skip(zipfile))]
pub(crate) async fn create<R, P>(zipfile: R, destination: P) -> Result<R>
where
    R: AsyncRead + AsyncSeek + Unpin + Send + 'static,
    P: AsRef<Path> + std::fmt::Debug,
{
    let destination = destination.as_ref().to_path_buf();
    spawn_blocking(move || {
        let mut conn = sqlite_create(&destination)?;
        let tx = conn.transaction()?;
        tx.execute_batch(
            "CREATE TABLE files (
                id INTEGER PRIMARY KEY,
                path TEXT UNIQUE,
                start INTEGER,
                end INTEGER,
                compression INTEGER
            );",
        )?;
        let mut bridge = SyncIoBridge::new(zipfile);
        let mut archive = zip::ZipArchive::new(&mut bridge)?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO files (path, start, end, compression) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for i in 0..archive.len() {
                let entry = archive.by_index(i)?;
                let start = entry
                    .data_start()
                    .ok_or_else(|| anyhow!("missing data_start in zip directory"))?;
                let end = start + entry.compressed_size() - 1;
                let compression = match entry.compression() {
                    zip::CompressionMethod::Bzip2 => CompressionAlgorithm::Bzip2 as i32,
                    zip::CompressionMethod::Deflated => CompressionAlgorithm::Deflate as i32,
                    c => bail!("unsupported compression algorithm {} in zip-file", c),
                };
                insert.execute(params![entry.name()?, start, end, compression])?;
            }
        }
        tx.execute_batch("CREATE INDEX idx_files_path ON files (path);")?;
        tx.commit()?;
        // VACUUM cannot run inside a transaction.
        conn.execute_batch("VACUUM;")?;
        drop(archive);
        Ok(bridge.into_inner())
    })
    .await
}

pub struct Index {
    // An owned guard can cross into the SQLite workers without requiring Connection
    // to be Sync. If a read is cancelled, the next read waits for its guard.
    conn: Arc<Mutex<Connection>>,
    threadpool: SqliteThreadPool,
}

impl Index {
    /// run an sqlite activity in the shared threadpool.
    /// Schedule immediately, so streaming consumers can receive batches before
    /// awaiting completion. Queued jobs skip work if their caller was cancelled.
    fn run_in_threadpool<T, F>(pool: &SqliteThreadPool, work: F) -> impl Future<Output = Result<T>>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T> + Send + 'static,
    {
        let span = tracing::Span::current();
        // Keep this span alive only until a worker picks up the job. Its duration
        // measures queueing, separately from SQLite execution.
        let queued = info_span!(parent: &span, "sqlite_worker_queue");
        let (send, recv) = oneshot::channel();
        pool.spawn(move || {
            drop(queued);
            let _span =
                info_span!(parent: &span, "sqlite_worker_execute", cancelled = send.is_closed())
                    .entered();
            if !send.is_closed() {
                // A panic drops the sender, returning an error to the caller.
                // The pool's panic handler prevents Rayon from aborting.
                let _ = send.send(work());
            }
        });
        async move {
            recv.await
                .context("archive index SQLite worker dropped its response")?
        }
    }

    /// Open a read-only, immutable index. Called only from a blocking task.
    #[instrument(skip(path), fields(index_path = %path.display()))]
    fn sqlite_open(path: &Path) -> Result<Connection> {
        let path = path::absolute(path)?;

        let mut uri = Url::from_file_path(&path)
            .map_err(|_| anyhow!("invalid archive index path: {}", path.display()))?;

        // Immutable indexes are replaced atomically, never modified in place.
        uri.query_pairs_mut().append_pair("immutable", "1");

        let conn = Connection::open_with_flags(
            uri.as_str(),
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        conn.pragma_update(None, "synchronous", "OFF")?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        conn.pragma_update(None, "query_only", true)?;
        conn.pragma_update(None, "mmap_size", ByteSize::mib(512).as_u64())?;
        conn.pragma_update(None, "cache_size", -4096)?;

        Ok(conn)
    }

    pub async fn open<P>(archive_index_path: P, threadpool: SqliteThreadPool) -> Result<Self>
    where
        P: AsRef<Path>,
    {
        let path = archive_index_path.as_ref().to_path_buf();
        let conn = Self::run_in_threadpool(&threadpool, move || Self::sqlite_open(&path)).await?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            threadpool,
        })
    }

    async fn read<T, F>(&mut self, read: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let mut conn = Arc::clone(&self.conn).lock_owned().await;
        Self::run_in_threadpool(&self.threadpool, move || read(&mut conn)).await
    }

    /// Number of files stored in the archive.
    #[cfg(any(test, feature = "testing"))]
    #[instrument(skip(self))]
    pub async fn file_count(&mut self) -> Result<u64> {
        self.read(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?))
            .await
    }

    #[instrument(skip(self))]
    pub async fn find<P>(&mut self, search_for: P) -> Result<Option<FileInfo>>
    where
        P: AsRef<Path> + fmt::Debug,
    {
        let path = search_for.as_ref();
        if path.is_absolute() {
            bail!("search path in archive index has to be relative");
        }
        let path = path
            .to_str()
            .ok_or_else(|| anyhow!("non-UTF-8 path in archive index lookup"))?
            .to_owned();
        self.read(move |conn| {
            let row: Option<(u64, u64, i32)> = conn
                .prepare_cached("SELECT start, end, compression FROM files WHERE path = ?1")?
                .query_row([&path], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .optional()
                .context("error fetching SQLite data")?;
            row.map(|(start, end, compression)| {
                Ok(FileInfo {
                    path: PathBuf::from(path),
                    range: start..=end,
                    compression: compression.try_into().map_err(|value| {
                        anyhow!("invalid compression algorithm '{value}' in database")
                    })?,
                })
            })
            .transpose()
        })
        .await
    }

    /// Stream bounded batches from a blocking SQLite query. Dropping the stream
    /// closes the receiver, allowing the producer to release its connection.
    fn read_rows<T, F>(
        &mut self,
        stream_span: tracing::Span,
        sql: &'static str,
        values: Vec<rusqlite::types::Value>,
        mut map_row: F,
    ) -> impl Stream<Item = Result<T>> + '_
    where
        T: Send + 'static,
        F: FnMut(&rusqlite::Row<'_>) -> Result<T> + Send + 'static,
    {
        // This span lives with the stream, rather than timing only its construction.
        // Never hold an entered span across a yield or await.
        try_stream! {
            let conn = Arc::clone(&self.conn).lock_owned().await;
            let (tx, mut rx) = mpsc::channel(2);
            // Bound memory and cross the thread boundary once per batch, not per row.
            let task = stream_span.in_scope(|| Self::run_in_threadpool(&self.threadpool, move || {
                let mut stmt = conn.prepare_cached(sql)?;
                let mut rows = stmt.query(rusqlite::params_from_iter(values))?;
                let mut batch = Vec::with_capacity(ROW_BATCH_SIZE);
                while let Some(row) = rows.next()? {
                    if tx.is_closed() {
                        return Ok(());
                    }
                    batch.push(map_row(row)?);
                    if batch.len() == ROW_BATCH_SIZE
                        && tx.blocking_send(mem::replace(
                            &mut batch, Vec::with_capacity(ROW_BATCH_SIZE),
                        )).is_err()
                    {
                        return Ok(()); // The consumer dropped the stream.
                    }
                }
                if !batch.is_empty() {
                    let _ = tx.blocking_send(batch);
                }
                Ok(())
            }));
            while let Some(batch) = rx.recv().await {
                for entry in batch {
                    yield entry;
                }
            }
            task.await?;
        }
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn list(&mut self) -> impl Stream<Item = Result<FileInfo>> + '_ {
        self.read_rows(
            info_span!("list"),
            "SELECT path, start, end, compression FROM files",
            Vec::new(),
            |row| {
                let path: String = row.get(0)?;
                let start: u64 = row.get(1)?;
                let end: u64 = row.get(2)?;
                let compression: i32 = row.get(3)?;
                Ok(FileInfo {
                    path: PathBuf::from(path),
                    range: start..=end,
                    compression: compression.try_into().map_err(|value| {
                        anyhow!("invalid compression algorithm '{value}' in database")
                    })?,
                })
            },
        )
    }

    /// get the folder contents inside the zip archive.
    /// * missing folder = list the root
    /// * given folder: just lists the files in there, and subfolders, but not their contents.
    ///
    /// You'll need this method when you build a file-browser for the archive, like
    /// in our source pages.
    pub fn folder_contents<P>(
        &mut self,
        folder: Option<P>,
    ) -> impl Stream<Item = Result<FolderEntry>> + '_
    where
        P: AsRef<Path> + std::fmt::Debug,
    {
        let prefix = folder
            .as_ref()
            .map(|f| format!("{}/", f.as_ref().to_string_lossy().trim_end_matches('/')));
        // Project and deduplicate immediate children in SQLite so that
        // descendants are neither allocated in Rust nor sent across threads.
        // instr(text, '/') returns the 1-based character position of the first slash.
        // Appending '/' makes it return length + 1 for a direct file, instead of 0.
        // substr(text, 1, length) stops at the end of the original text:
        // "src/lib.rs" becomes "src/", but "lib.rs" stays "lib.rs".
        // Retaining the slash distinguishes a directory from a same-named file.
        let (sql, values) = if let Some(prefix) = prefix {
            // substr(path, start) removes the folder prefix using a 1-based
            // character position, so count characters rather than UTF-8 bytes.
            let start = prefix.chars().count() as i64 + 1;
            // Filter the original path with an indexed range; the functions only
            // project the immediate child. Replacing the prefix's trailing '/'
            // with the next ASCII character gives an exclusive upper bound
            // that includes every possible suffix without interpreting wildcards.
            let upper_bound = format!("{}0", prefix.trim_end_matches('/'));
            (
                "SELECT DISTINCT substr(relative_path, 1, instr(relative_path || '/', '/')) AS child
                 FROM (
                     SELECT substr(path, ?1) AS relative_path
                     FROM files WHERE path >= ?2 AND path < ?3
                 )",
                vec![rusqlite::types::Value::Integer(start), prefix.into(), upper_bound.into()],
            )
        } else {
            (
                "SELECT DISTINCT substr(path, 1, instr(path || '/', '/')) AS child FROM files",
                Vec::new(),
            )
        };
        self.read_rows(info_span!("folder_contents", ?folder), sql, values, |row| {
            let child: String = row.get(0)?;
            Ok(FolderEntry::from_path(&child))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{blob::StreamingBlob, storage::non_blocking::ZIP_BUFFER_SIZE};
    use chrono::Utc;
    use docs_rs_config::AppConfig as _;
    use docs_rs_opentelemetry::testing::TestMetrics;
    use std::{
        collections::{HashMap, HashSet},
        io::Cursor,
        ops::Deref,
        pin::Pin,
        sync::{Arc, LazyLock},
    };
    use zip::write::SimpleFileOptions;

    static TEST_THREADPOOL: LazyLock<SqliteThreadPool> =
        LazyLock::new(|| new_threadpool(2).expect("create test SQLite pool"));

    /// Open another connection using the shared unit-test worker pool.
    pub(crate) async fn open_archive_index(index_path: impl AsRef<Path>) -> anyhow::Result<Index> {
        Index::open(index_path, TEST_THREADPOOL.clone()).await
    }

    #[tokio::test]
    async fn cancelled_running_read_keeps_connection_until_worker_finishes() -> Result<()> {
        let path = tempfile::NamedTempFile::new()?.into_temp_path();
        create(create_test_archive(1).await?, &path).await?;
        // Leave a worker free so a subsequent read waits for the connection,
        // rather than merely waiting for the pool's only worker.
        let pool = new_threadpool(2)?;
        let mut index = Index::open(&path, pool).await?;
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut first = Box::pin(index.read(move |_| {
            let _ = started_tx.send(());
            release_rx.recv()?;
            Ok(())
        }));
        assert!(futures_util::poll!(&mut first).is_pending());
        tokio::time::timeout(Duration::from_secs(5), started_rx).await??;
        drop(first);
        assert!(
            index.conn.try_lock().is_err(),
            "cancelling the caller must not release the running worker's connection"
        );

        let mut next = Box::pin(index.find("testfile0"));
        assert!(futures_util::poll!(&mut next).is_pending());
        release_tx.send(())?;
        assert!(
            tokio::time::timeout(Duration::from_secs(5), next)
                .await??
                .is_some()
        );
        Ok(())
    }

    #[tokio::test]
    async fn cancelled_queued_job_does_not_run() -> Result<()> {
        let pool = new_threadpool(1)?;
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let busy = Index::run_in_threadpool(&pool, move || {
            let _ = started_tx.send(());
            release_rx.recv()?;
            Ok(())
        });
        started_rx.await?;
        let (called_tx, called_rx) = oneshot::channel();
        let cancelled = Index::run_in_threadpool(&pool, move || {
            let _ = called_tx.send(());
            Ok(())
        });
        drop(cancelled);
        release_tx.send(())?;
        busy.await?;
        // Skipping the queued closure drops its captured sender.
        assert!(
            tokio::time::timeout(Duration::from_secs(5), called_rx)
                .await?
                .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn panicking_job_does_not_stop_workers() -> Result<()> {
        let pool = new_threadpool(1)?;
        let result = Index::run_in_threadpool(&pool, || -> Result<()> {
            panic!("test SQLite job panic");
        })
        .await;
        assert!(result.is_err());
        assert_eq!(Index::run_in_threadpool(&pool, || Ok(42)).await?, 42);
        Ok(())
    }

    const CORRUPT_SQLITE_INDEX: &[u8] = b"not-an-sqlite-index";

    #[tokio::test]
    async fn index_open_escapes_uri_characters_in_filename() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("index ?#% ü.sqlite");
        create(create_test_archive(1).await?, &path).await?;

        let mut index = open_archive_index(&path).await?;
        assert!(index.find("testfile0").await?.is_some());
        let err = index
            .read(|conn| {
                conn.execute("DELETE FROM files", [])?;
                Ok(())
            })
            .await
            .unwrap_err();
        assert!(matches!(
            err.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(code, _))
                if code.code == rusqlite::ErrorCode::ReadOnly
        ));
        Ok(())
    }

    #[tokio::test]
    async fn opening_missing_index_does_not_create_file() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("missing.index");
        assert!(open_archive_index(&path).await.is_err());
        assert!(!path.exists());
        Ok(())
    }

    #[test_case::test_case(false; "full index")]
    #[test_case::test_case(true; "root folder")]
    #[tokio::test]
    async fn dropping_read_stream_releases_connection(folder: bool) -> Result<()> {
        fn paths(
            index: &mut Index,
            folder: bool,
        ) -> Pin<Box<dyn Stream<Item = Result<String>> + '_>> {
            if folder {
                Box::pin(
                    index
                        .folder_contents(None::<&Path>)
                        .map_ok(|entry| entry.name().to_owned()),
                )
            } else {
                Box::pin(
                    index
                        .list()
                        .map_ok(|entry| entry.path.to_str().unwrap().to_owned()),
                )
            }
        }

        let path = tempfile::NamedTempFile::new()?.into_temp_path();
        let count = (ROW_BATCH_SIZE * 8 + 3) as u32;
        create(create_test_archive(count).await?, &path).await?;
        let mut index = open_archive_index(&path).await?;
        {
            let mut stream = paths(&mut index, folder);
            assert!(stream.try_next().await?.is_some());
            // Drop before consuming all batches, unblocking the producer even
            // if its bounded output channel is full.
        }
        let found = tokio::time::timeout(Duration::from_secs(5), index.find("testfile0")).await??;
        assert!(found.is_some());
        // Also consume every batch, including the final partial batch.
        let paths = paths(&mut index, folder).try_collect::<Vec<_>>().await?;
        let actual: HashSet<_> = paths.iter().cloned().collect();
        let expected: HashSet<_> = (0..count).map(|n| format!("testfile{n}")).collect();
        assert_eq!(paths.len(), count as usize);
        assert_eq!(actual, expected);
        Ok(())
    }

    /// Creates a test archive from a list of (path, content) pairs.
    async fn create_archive_from_entries(
        entries: Vec<(&'static str, &'static [u8])>,
    ) -> Result<fs::File> {
        spawn_blocking(move || {
            use std::io::Write as _;
            let tf = tempfile::tempfile()?;
            let mut archive = zip::ZipWriter::new(tf);
            let options = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Bzip2)
                .compression_level(Some(1));
            for (path, content) in entries {
                archive.start_file(path, options)?;
                archive.write_all(content)?;
            }
            Ok(archive.finish()?)
        })
        .await
        .map(fs::File::from_std)
    }

    async fn create_test_archive(file_count: u32) -> Result<fs::File> {
        create_test_archive_with_compression(file_count, zip::CompressionMethod::Deflated).await
    }

    async fn create_test_archive_with_compression(
        file_count: u32,
        compression: zip::CompressionMethod,
    ) -> Result<fs::File> {
        let writer = spawn_blocking(move || {
            use std::io::Write as _;

            let tf = tempfile::tempfile()?;

            let objectcontent: Vec<u8> = (0..255).collect();

            let mut archive =
                zip::ZipWriter::new(std::io::BufWriter::with_capacity(ZIP_BUFFER_SIZE, tf));
            for i in 0..file_count {
                archive.start_file(
                    format!("testfile{i}"),
                    SimpleFileOptions::default()
                        .compression_method(compression)
                        .compression_level(Some(1)),
                )?;
                archive.write_all(&objectcontent)?;
            }
            Ok(archive.finish()?)
        })
        .await?;

        Ok(fs::File::from_std(writer.into_inner()?))
    }

    struct FakeDownloader {
        indices: HashMap<String, Vec<u8>>,
        download_count: std::sync::Mutex<HashMap<String, usize>>,
        delay: Option<std::time::Duration>,
    }

    impl FakeDownloader {
        fn new() -> Self {
            Self {
                indices: HashMap::new(),
                download_count: std::sync::Mutex::new(HashMap::new()),
                delay: None,
            }
        }

        fn with_delay(delay: std::time::Duration) -> Self {
            let mut downloader = Self::new();
            downloader.delay = Some(delay);
            downloader
        }

        fn download_count(&self, remote_index_path: &str) -> usize {
            let download_count = self.download_count.lock().unwrap();
            *download_count.get(remote_index_path).unwrap_or(&0)
        }
    }

    impl Downloader for FakeDownloader {
        fn fetch_archive_index<'a>(
            &'a self,
            remote_index_path: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<StreamingBlob>> + Send + 'a>> {
            Box::pin(async move {
                if let Some(delay) = self.delay {
                    tokio::time::sleep(delay).await;
                }

                let mut fetch_count = self.download_count.lock().unwrap();
                fetch_count
                    .entry(remote_index_path.to_string())
                    .and_modify(|count| *count += 1)
                    .or_insert(1);

                let content = self
                    .indices
                    .get(remote_index_path)
                    .cloned()
                    .ok_or_else(|| anyhow!("missing index fixture for {remote_index_path}"))?;

                Ok(StreamingBlob {
                    path: remote_index_path.to_string(),
                    mime: mime::APPLICATION_OCTET_STREAM,
                    date_updated: Utc::now(),
                    etag: None,
                    compression: None,
                    content_length: Some(content.len()),
                    content: Box::new(Cursor::new(content)),
                })
            })
        }
    }

    struct FlakyDownloader {
        remote_index_path: String,
        payload: Vec<u8>,
        fail_until: usize,
        fetch_count: std::sync::Mutex<usize>,
    }

    impl FlakyDownloader {
        fn new(remote_index_path: String, payload: Vec<u8>, fail_until: usize) -> Self {
            Self {
                remote_index_path,
                payload,
                fail_until,
                fetch_count: std::sync::Mutex::new(0),
            }
        }

        fn fetch_count(&self) -> usize {
            *self.fetch_count.lock().unwrap()
        }
    }

    impl Downloader for FlakyDownloader {
        fn fetch_archive_index<'a>(
            &'a self,
            remote_index_path: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<StreamingBlob>> + Send + 'a>> {
            Box::pin(async move {
                if remote_index_path != self.remote_index_path {
                    bail!(
                        "unexpected remote index path: expected {}, got {remote_index_path}",
                        self.remote_index_path
                    );
                }

                let mut fetch_count = self.fetch_count.lock().unwrap();
                *fetch_count += 1;
                if *fetch_count <= self.fail_until {
                    bail!("synthetic download failure {fetch_count}");
                }

                let content = self.payload.clone();
                Ok(StreamingBlob {
                    path: remote_index_path.to_string(),
                    mime: mime::APPLICATION_OCTET_STREAM,
                    date_updated: Utc::now(),
                    etag: None,
                    compression: None,
                    content_length: Some(content.len()),
                    content: Box::new(Cursor::new(content)),
                })
            })
        }
    }

    async fn create_index(file_count: u32) -> Result<tempfile::TempPath> {
        let tf = create_test_archive(file_count).await?;
        let tempfile = tempfile::NamedTempFile::new()?.into_temp_path();
        create(tf, &tempfile).await?;
        Ok(tempfile)
    }

    async fn create_index_bytes(file_count: u32) -> Result<Vec<u8>> {
        let tempfile = create_index(file_count).await?;
        fs::read(&tempfile).await.map_err(Into::into)
    }

    struct TestEnv {
        _collected_metrics: TestMetrics,
        config: Arc<ArchiveIndexCacheConfig>,
        cache: Cache,
    }

    impl Deref for TestEnv {
        type Target = Cache;

        fn deref(&self) -> &Self::Target {
            &self.cache
        }
    }

    async fn test_cache() -> Result<TestEnv> {
        let config = Arc::new(ArchiveIndexCacheConfig::test_config()?);
        let meter_provider = TestMetrics::new();
        let cache = Cache::new_with_backfill(config.clone(), meter_provider.provider()).await?;

        Ok(TestEnv {
            _collected_metrics: meter_provider,
            cache,
            config,
        })
    }

    #[tokio::test]
    async fn index_create_save_load_sqlite() -> Result<()> {
        let tf = create_test_archive(1).await?;

        let tempfile = tempfile::NamedTempFile::new()?.into_temp_path();
        create(tf, &tempfile).await?;

        let mut index = open_archive_index(&tempfile).await?;
        let fi = index.find("testfile0").await?.unwrap();

        assert_eq!(fi.compression, CompressionAlgorithm::Deflate);

        assert!(index.find("some_other_file",).await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn index_create_save_load_sqlite_legacy_bzip2() -> Result<()> {
        let tf = create_test_archive_with_compression(1, zip::CompressionMethod::Bzip2).await?;

        let tempfile = tempfile::NamedTempFile::new()?.into_temp_path();
        create(tf, &tempfile).await?;

        let mut index = open_archive_index(&tempfile).await?;
        let fi = index.find("testfile0").await?.unwrap();

        assert_eq!(fi.compression, CompressionAlgorithm::Bzip2);

        assert!(index.find("some_other_file").await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn empty_archive() -> Result<()> {
        let tf = create_test_archive(0).await?;

        let tempfile = tempfile::NamedTempFile::new()?.into_temp_path();
        create(tf, &tempfile).await?;

        let mut index = open_archive_index(&tempfile).await?;
        assert_eq!(index.file_count().await?, 0);

        Ok(())
    }

    #[tokio::test]
    async fn archive_with_more_than_65k_files() -> Result<()> {
        let tf = create_test_archive(100_000).await?;

        let tempfile = tempfile::NamedTempFile::new()?.into_temp_path();
        create(tf, &tempfile).await?;

        let mut index = open_archive_index(&tempfile).await?;
        assert_eq!(index.file_count().await?, 100_000);

        Ok(())
    }

    #[tokio::test]
    async fn outdated_local_archive_index_gets_redownloaded() -> Result<()> {
        let cache = test_cache().await?;

        const LATEST_BUILD_ID: Option<BuildId> = Some(BuildId(42));
        const ARCHIVE_NAME: &str = "test.zip";
        const FILE_IN_ARCHIVE: &str = "testfile0";

        let cache_file = cache.local_index_path(ARCHIVE_NAME, LATEST_BUILD_ID);
        let remote_index_path = format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}");
        let mut downloader = FakeDownloader::new();
        downloader
            .indices
            .insert(remote_index_path.clone(), create_index_bytes(2).await?);

        assert!(!fs::try_exists(&cache_file).await?);
        assert!(
            cache
                .find(ARCHIVE_NAME, LATEST_BUILD_ID, FILE_IN_ARCHIVE, &downloader)
                .await?
                .is_some()
        );
        assert!(fs::try_exists(&cache_file).await?);
        assert_eq!(downloader.download_count(&remote_index_path), 1);

        // Simulate local cache corruption and ensure Cache::find repairs it.
        fs::write(&cache_file, CORRUPT_SQLITE_INDEX).await?;
        assert!(
            cache
                .find(ARCHIVE_NAME, LATEST_BUILD_ID, FILE_IN_ARCHIVE, &downloader)
                .await?
                .is_some()
        );
        assert_eq!(downloader.download_count(&remote_index_path), 2);

        Ok(())
    }

    #[tokio::test]
    async fn find_uses_local_cache_without_downloading() -> Result<()> {
        let cache = test_cache().await?;
        const LATEST_BUILD_ID: Option<BuildId> = Some(BuildId(7));
        const ARCHIVE_NAME: &str = "test.zip";
        const FILE_IN_ARCHIVE: &str = "testfile0";

        let cache_file = cache.local_index_path(ARCHIVE_NAME, LATEST_BUILD_ID);
        fs::create_dir_all(cache_file.parent().unwrap()).await?;
        fs::write(&cache_file, create_index_bytes(1).await?).await?;

        let downloader = FakeDownloader::new();
        let result = cache
            .find(ARCHIVE_NAME, LATEST_BUILD_ID, FILE_IN_ARCHIVE, &downloader)
            .await?;
        assert!(result.is_some());
        assert_eq!(
            downloader.download_count(&format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}")),
            0
        );
        assert!(cache.manager.get(&cache_file).await.is_some());

        Ok(())
    }

    #[test_case::test_case(false; "missing file")]
    #[test_case::test_case(true; "corrupt file")]
    #[tokio::test]
    async fn find_index_inner_repairs_stale_manager_entry(corrupt: bool) -> Result<()> {
        let cache = test_cache().await?;
        const BUILD_ID: Option<BuildId> = Some(BuildId(7));
        const ARCHIVE_NAME: &str = "stale-manager.zip";
        let local_index = cache.local_index_path(ARCHIVE_NAME, BUILD_ID);
        let remote_index_path = format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}");
        let index_bytes = create_index_bytes(1).await?;
        fs::write(&local_index, &index_bytes).await?;
        let mut downloader = FakeDownloader::new();
        downloader
            .indices
            .insert(remote_index_path.clone(), index_bytes);

        cache
            .find(ARCHIVE_NAME, BUILD_ID, "testfile0", &downloader)
            .await?;
        assert_eq!(downloader.download_count(&remote_index_path), 0);
        assert!(cache.manager.get(&local_index).await.is_some());

        if corrupt {
            fs::write(&local_index, CORRUPT_SQLITE_INDEX).await?;
        } else {
            fs::remove_file(&local_index).await?;
        }

        // Repair must happen within one attempt, even though Moka still has an
        // entry. Calling the inner method excludes the outer purge/retry loop.
        let mut index = cache
            .find_index_inner(ARCHIVE_NAME, BUILD_ID, &downloader)
            .await?;
        assert!(index.find("testfile0").await?.is_some());
        assert_eq!(downloader.download_count(&remote_index_path), 1);
        assert!(cache.manager.get(&local_index).await.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn delayed_repair_reuses_replacement() -> Result<()> {
        let cache = test_cache().await?;
        const BUILD_ID: Option<BuildId> = Some(BuildId(7));
        const ARCHIVE_NAME: &str = "delayed-repair.zip";
        let local_index = cache.local_index_path(ARCHIVE_NAME, BUILD_ID);
        let remote_index_path = format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}");
        fs::write(&local_index, CORRUPT_SQLITE_INDEX).await?;

        // Pause request B after its failed fast-path open, then let request A
        // complete a repair before B enters the slow path. Ordering these steps
        // explicitly exercises the race without relying on scheduler timing.
        assert!(open_archive_index(&local_index).await.is_err());
        let mut downloader = FakeDownloader::new();
        downloader
            .indices
            .insert(remote_index_path.clone(), create_index_bytes(1).await?);
        let mut first_index = cache
            .find_index_inner(ARCHIVE_NAME, BUILD_ID, &downloader)
            .await?;
        assert_eq!(downloader.download_count(&remote_index_path), 1);
        assert!(cache.manager.get(&local_index).await.is_some());

        // B must reuse A's replacement even after invalidating its fresh manager
        // entry. No remote fixture is available to hide an unnecessary download.
        let delayed_downloader = FakeDownloader::new();
        let mut second_index = cache
            .repair_index(&local_index, &remote_index_path, &delayed_downloader)
            .await?;
        assert!(first_index.find("testfile0").await?.is_some());
        assert!(second_index.find("testfile0").await?.is_some());
        assert_eq!(delayed_downloader.download_count(&remote_index_path), 0);
        assert!(cache.manager.get(&local_index).await.is_some());
        Ok(())
    }

    #[tokio::test]
    async fn find_downloads_when_local_cache_missing() -> Result<()> {
        let cache = test_cache().await?;
        const LATEST_BUILD_ID: Option<BuildId> = Some(BuildId(7));
        const ARCHIVE_NAME: &str = "test.zip";
        const FILE_IN_ARCHIVE: &str = "testfile0";

        let remote_index_path = format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}");
        let mut downloader = FakeDownloader::new();
        downloader
            .indices
            .insert(remote_index_path.clone(), create_index_bytes(1).await?);

        let result = cache
            .find(ARCHIVE_NAME, LATEST_BUILD_ID, FILE_IN_ARCHIVE, &downloader)
            .await?;
        assert!(result.is_some());
        assert_eq!(downloader.download_count(&remote_index_path), 1);
        assert!(fs::try_exists(cache.local_index_path(ARCHIVE_NAME, LATEST_BUILD_ID)).await?);

        Ok(())
    }

    #[tokio::test]
    async fn find_returns_none_for_missing_entry() -> Result<()> {
        let cache = test_cache().await?;
        const LATEST_BUILD_ID: Option<BuildId> = Some(BuildId(7));
        const ARCHIVE_NAME: &str = "test.zip";

        let remote_index_path = format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}");
        let mut downloader = FakeDownloader::new();
        downloader
            .indices
            .insert(remote_index_path.clone(), create_index_bytes(1).await?);

        let result = cache
            .find(ARCHIVE_NAME, LATEST_BUILD_ID, "does-not-exist", &downloader)
            .await?;
        assert!(result.is_none());
        assert_eq!(downloader.download_count(&remote_index_path), 1);

        Ok(())
    }

    #[tokio::test]
    async fn missing_remote_index_is_not_retried_or_purged() -> Result<()> {
        let cache = test_cache().await?;
        let archive = "missing-remote.zip";
        let path = cache.local_index_path(archive, None);
        let replacement = create_index_bytes(1).await?;
        fs::write(&path, &replacement).await?;
        let mut calls = 0;
        let result = cache
            .retry_with_purge::<(), _, _>(archive, None, || {
                calls += 1;
                // Keep the error recognizable even when a caller adds context.
                std::future::ready(Err(
                    anyhow!(PathNotFoundError).context("fetching remote index")
                ))
            })
            .await;

        assert!(result.unwrap_err().is::<PathNotFoundError>());
        assert_eq!(calls, 1);
        assert_eq!(fs::read(&path).await?, replacement);
        Ok(())
    }

    #[test_case::test_case(1; "one failed open")]
    #[test_case::test_case(2; "repeated failed opens")]
    #[tokio::test]
    async fn retry_preserves_replacement_after_failed_open(failed_opens: usize) -> Result<()> {
        let cache = test_cache().await?;
        const BUILD_ID: Option<BuildId> = Some(BuildId(7));
        const ARCHIVE_NAME: &str = "retry-replacement.zip";
        let local_index = cache.local_index_path(ARCHIVE_NAME, BUILD_ID);
        let replacement = create_index_bytes(1).await?;
        // A retry must use the replacement without needing remote storage.
        let downloader = FakeDownloader::new();
        let mut calls = 0;
        let (mut index, attempts) = cache
            .retry_with_purge(ARCHIVE_NAME, BUILD_ID, || {
                calls += 1;
                let call = calls;
                let cache = &cache;
                let local_index = &local_index;
                let replacement = &replacement;
                let downloader = &downloader;
                async move {
                    if call <= failed_opens {
                        let err = open_archive_index(local_index)
                            .await
                            .err()
                            .context("initial open should fail")?;
                        if call == failed_opens {
                            // Another request publishes a replacement before
                            // this failed open reaches the retry loop.
                            fs::write(local_index, replacement).await?;
                        }
                        return Err(err);
                    }
                    cache
                        .find_index_inner(ARCHIVE_NAME, BUILD_ID, downloader)
                        .await
                }
            })
            .await?;

        assert_eq!(attempts, failed_opens + 1);
        assert!(index.find("testfile0").await?.is_some());
        assert_eq!(fs::read(&local_index).await?, replacement);
        assert_eq!(
            downloader.download_count(&format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}")),
            0
        );
        Ok(())
    }

    #[tokio::test]
    async fn retry_purges_after_persistent_query_failure() -> Result<()> {
        let cache = test_cache().await?;
        const BUILD_ID: Option<BuildId> = Some(BuildId(7));
        const ARCHIVE_NAME: &str = "retry-query-failure.zip";
        let local_index = cache.local_index_path(ARCHIVE_NAME, BUILD_ID);
        // A valid SQLite database with the wrong schema opens successfully, so
        // the lookup must eventually purge it after the non-destructive retry.
        let path = local_index.clone();
        spawn_blocking(move || {
            sqlite_create(&path)?.execute_batch("CREATE TABLE unrelated (id INTEGER);")?;
            Ok(())
        })
        .await?;
        let remote_index_path = format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}");
        let mut downloader = FakeDownloader::new();
        downloader
            .indices
            .insert(remote_index_path.clone(), create_index_bytes(1).await?);

        let (found, attempts) = cache
            .retry_with_purge(ARCHIVE_NAME, BUILD_ID, || async {
                cache
                    .find_index_inner(ARCHIVE_NAME, BUILD_ID, &downloader)
                    .await?
                    .find("testfile0")
                    .await
            })
            .await?;
        assert!(found.is_some());
        assert_eq!(attempts, 3);
        assert_eq!(downloader.download_count(&remote_index_path), 1);
        Ok(())
    }

    #[tokio::test]
    async fn find_retries_then_errors() -> Result<()> {
        let cache = test_cache().await?;
        const LATEST_BUILD_ID: Option<BuildId> = Some(BuildId(7));
        const ARCHIVE_NAME: &str = "test.zip";

        let remote_index_path = format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}");
        let mut downloader = FakeDownloader::new();
        downloader
            .indices
            .insert(remote_index_path.clone(), CORRUPT_SQLITE_INDEX.to_vec());

        let err = cache
            .find(ARCHIVE_NAME, LATEST_BUILD_ID, "testfile0", &downloader)
            .await
            .unwrap_err();

        assert!(matches!(
            err.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(code, _))
                if code.code == rusqlite::ErrorCode::NotADatabase
        ));
        assert_eq!(
            downloader.download_count(&remote_index_path),
            REPAIR_ATTEMPTS
        );

        Ok(())
    }

    #[tokio::test]
    async fn corrupted_local_index_uses_first_attempt_for_redownload() -> Result<()> {
        let cache = test_cache().await?;
        const LATEST_BUILD_ID: Option<BuildId> = Some(BuildId(808));
        const ARCHIVE_NAME: &str = "corrupt-first-attempt-redownload.zip";
        const FILE_IN_ARCHIVE: &str = "testfile0";

        let cache_file = cache.local_index_path(ARCHIVE_NAME, LATEST_BUILD_ID);
        fs::create_dir_all(cache_file.parent().unwrap()).await?;
        fs::write(&cache_file, CORRUPT_SQLITE_INDEX).await?;

        let remote_index_path = format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}");
        let downloader = FlakyDownloader::new(
            remote_index_path,
            create_index_bytes(1).await?,
            REPAIR_ATTEMPTS - 1,
        );

        let result = cache
            .find(ARCHIVE_NAME, LATEST_BUILD_ID, FILE_IN_ARCHIVE, &downloader)
            .await?;
        assert!(result.is_some());
        assert_eq!(downloader.fetch_count(), REPAIR_ATTEMPTS);

        Ok(())
    }

    #[tokio::test]
    async fn purge_removes_index_wal_and_shm() -> Result<()> {
        let cache = test_cache().await?;
        const LATEST_BUILD_ID: Option<BuildId> = Some(BuildId(7));
        const ARCHIVE_NAME: &str = "test.zip";

        let local_index = cache.local_index_path(ARCHIVE_NAME, LATEST_BUILD_ID);
        let wal = local_index.with_extension(format!("{ARCHIVE_INDEX_FILE_EXTENSION}-wal"));
        let shm = local_index.with_extension(format!("{ARCHIVE_INDEX_FILE_EXTENSION}-shm"));

        fs::create_dir_all(local_index.parent().unwrap()).await?;
        fs::write(&local_index, b"index").await?;
        fs::write(&wal, b"wal").await?;
        fs::write(&shm, b"shm").await?;

        cache.purge(ARCHIVE_NAME, LATEST_BUILD_ID).await?;

        assert!(!fs::try_exists(&local_index).await?);
        assert!(!fs::try_exists(&wal).await?);
        assert!(!fs::try_exists(&shm).await?);

        Ok(())
    }

    #[tokio::test]
    async fn purge_is_idempotent_when_files_missing() -> Result<()> {
        let cache = test_cache().await?;
        cache.purge("missing.zip", Some(BuildId(7))).await?;
        cache.purge("missing.zip", Some(BuildId(7))).await?;

        Ok(())
    }

    #[tokio::test]
    async fn manager_capacity_eviction_removes_index_wal_and_shm() -> Result<()> {
        let cache = test_cache().await?;
        let local_index = cache.local_index_path("listener-remove.zip", Some(BuildId(17)));
        let wal = local_index.with_extension(format!("{ARCHIVE_INDEX_FILE_EXTENSION}-wal"));
        let shm = local_index.with_extension(format!("{ARCHIVE_INDEX_FILE_EXTENSION}-shm"));

        fs::create_dir_all(local_index.parent().unwrap()).await?;
        fs::write(&local_index, b"index").await?;
        fs::write(&wal, b"wal").await?;
        fs::write(&shm, b"shm").await?;

        cache
            .manager
            .insert(
                local_index.clone(),
                Arc::new(Entry::from_size(ByteSize::mb(
                    (cache.config.max_size.as_mb() + 1.0) as u64,
                ))),
            )
            .await;

        cache.flush().await?;

        assert!(!fs::try_exists(&local_index).await?);
        assert!(!fs::try_exists(&wal).await?);
        assert!(!fs::try_exists(&shm).await?);

        Ok(())
    }

    #[tokio::test]
    async fn purge_invalidates_manager_so_next_find_redownloads() -> Result<()> {
        let cache = test_cache().await?;
        const LATEST_BUILD_ID: Option<BuildId> = Some(BuildId(23));
        const ARCHIVE_NAME: &str = "purge-redownload.zip";
        const FILE_IN_ARCHIVE: &str = "testfile0";

        let remote_index_path = format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}");
        let mut downloader = FakeDownloader::new();
        downloader
            .indices
            .insert(remote_index_path.clone(), create_index_bytes(1).await?);

        assert!(
            cache
                .find(ARCHIVE_NAME, LATEST_BUILD_ID, FILE_IN_ARCHIVE, &downloader)
                .await?
                .is_some()
        );
        assert_eq!(downloader.download_count(&remote_index_path), 1);

        cache.purge(ARCHIVE_NAME, LATEST_BUILD_ID).await?;

        assert!(
            cache
                .find(ARCHIVE_NAME, LATEST_BUILD_ID, FILE_IN_ARCHIVE, &downloader)
                .await?
                .is_some()
        );
        assert_eq!(downloader.download_count(&remote_index_path), 2);

        Ok(())
    }

    #[tokio::test]
    async fn purge_for_build_id_does_not_invalidate_other_build_id() -> Result<()> {
        let cache = test_cache().await?;
        const BUILD_ID_A: Option<BuildId> = Some(BuildId(101));
        const BUILD_ID_B: Option<BuildId> = Some(BuildId(202));
        const ARCHIVE_NAME: &str = "build-id-isolation.zip";
        const FILE_IN_ARCHIVE: &str = "testfile0";

        let local_a = cache.local_index_path(ARCHIVE_NAME, BUILD_ID_A);
        let local_b = cache.local_index_path(ARCHIVE_NAME, BUILD_ID_B);
        fs::create_dir_all(local_a.parent().unwrap()).await?;
        let index_bytes = create_index_bytes(1).await?;
        fs::write(&local_a, &index_bytes).await?;
        fs::write(&local_b, &index_bytes).await?;

        let remote_index_path = format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}");
        let mut downloader = FakeDownloader::new();
        downloader
            .indices
            .insert(remote_index_path.clone(), index_bytes.clone());

        assert!(
            cache
                .find(ARCHIVE_NAME, BUILD_ID_A, FILE_IN_ARCHIVE, &downloader)
                .await?
                .is_some()
        );
        assert!(
            cache
                .find(ARCHIVE_NAME, BUILD_ID_B, FILE_IN_ARCHIVE, &downloader)
                .await?
                .is_some()
        );
        assert_eq!(downloader.download_count(&remote_index_path), 0);

        cache.purge(ARCHIVE_NAME, BUILD_ID_A).await?;

        assert!(
            cache
                .find(ARCHIVE_NAME, BUILD_ID_A, FILE_IN_ARCHIVE, &downloader)
                .await?
                .is_some()
        );
        assert_eq!(downloader.download_count(&remote_index_path), 1);

        assert!(
            cache
                .find(ARCHIVE_NAME, BUILD_ID_B, FILE_IN_ARCHIVE, &downloader)
                .await?
                .is_some()
        );
        assert_eq!(downloader.download_count(&remote_index_path), 1);

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn purge_during_inflight_find_does_not_break_recovery() -> Result<()> {
        let cache = Arc::new(test_cache().await?);
        const LATEST_BUILD_ID: Option<BuildId> = Some(BuildId(303));
        const ARCHIVE_NAME: &str = "inflight-purge.zip";
        const FILE_IN_ARCHIVE: &str = "testfile0";

        let remote_index_path = format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}");
        let mut downloader = FakeDownloader::with_delay(std::time::Duration::from_millis(150));
        downloader
            .indices
            .insert(remote_index_path.clone(), create_index_bytes(1).await?);
        let downloader = Arc::new(downloader);

        let find_task = {
            let cache = cache.clone();
            let downloader = downloader.clone();
            tokio::spawn(async move {
                cache
                    .find(
                        ARCHIVE_NAME,
                        LATEST_BUILD_ID,
                        FILE_IN_ARCHIVE,
                        downloader.as_ref(),
                    )
                    .await
            })
        };

        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        cache.purge(ARCHIVE_NAME, LATEST_BUILD_ID).await?;

        let result = find_task.await??;
        assert!(result.is_some());
        assert!(downloader.download_count(&remote_index_path) <= 2);

        let second = cache
            .find(
                ARCHIVE_NAME,
                LATEST_BUILD_ID,
                FILE_IN_ARCHIVE,
                downloader.as_ref(),
            )
            .await?;
        assert!(second.is_some());
        assert!(downloader.download_count(&remote_index_path) <= 2);

        Ok(())
    }

    #[tokio::test]
    async fn backfill_then_find_uses_backfilled_entry_without_download_when_file_exists()
    -> Result<()> {
        let cache = test_cache().await?;
        const LATEST_BUILD_ID: Option<BuildId> = Some(BuildId(404));
        const ARCHIVE_NAME: &str = "backfill-preexisting.zip";
        const FILE_IN_ARCHIVE: &str = "testfile0";

        let local_index = cache.config.path.join(format!(
            "{ARCHIVE_NAME}.{}.{ARCHIVE_INDEX_FILE_EXTENSION}",
            LATEST_BUILD_ID.unwrap().0
        ));
        fs::create_dir_all(local_index.parent().unwrap()).await?;
        fs::write(&local_index, create_index_bytes(1).await?).await?;

        cache.backfill().await?;

        assert!(cache.manager.get(&local_index).await.is_some());

        let downloader = FakeDownloader::new();
        let result = cache
            .find(ARCHIVE_NAME, LATEST_BUILD_ID, FILE_IN_ARCHIVE, &downloader)
            .await?;
        assert!(result.is_some());
        assert_eq!(
            downloader.download_count(&format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}")),
            0
        );

        Ok(())
    }

    #[tokio::test]
    async fn backfill_skips_non_index_files() -> Result<()> {
        let cache = test_cache().await?;
        let non_index_file = cache.config.path.join("not-an-index.tmp");
        fs::create_dir_all(&cache.config.path).await?;
        fs::write(&non_index_file, b"junk").await?;

        assert!(cache.manager.get(&non_index_file).await.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn download_archive_index_overwrites_existing_file() -> Result<()> {
        let cache = test_cache().await?;
        let local_index = cache.local_index_path("test.zip", Some(BuildId(7)));
        fs::create_dir_all(local_index.parent().unwrap()).await?;
        fs::write(&local_index, b"old").await?;

        let remote_index_path = "test.zip.index";
        let mut downloader = FakeDownloader::new();
        downloader
            .indices
            .insert(remote_index_path.to_string(), create_index_bytes(1).await?);

        cache
            .download_archive_index(&downloader, &local_index, remote_index_path)
            .await?;

        let written = fs::read(&local_index).await?;
        assert!(!written.is_empty());
        assert_ne!(written, b"old");

        Ok(())
    }

    #[tokio::test]
    async fn broken_indexes() -> Result<()> {
        let temp_path = tempfile::NamedTempFile::new()?.into_temp_path();
        fs::write(&temp_path, CORRUPT_SQLITE_INDEX).await?;

        assert!(open_archive_index(&temp_path).await.is_err());

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_find_triggers_single_download_per_index() -> Result<()> {
        let cache = test_cache().await?;
        let cache = Arc::new(cache);
        const N: usize = 16;
        const LATEST_BUILD_ID: Option<BuildId> = Some(BuildId(7));
        const ARCHIVE_NAME: &str = "test.zip";
        const FILE_IN_ARCHIVE: &str = "testfile0";

        let remote_index_path = format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}");
        let mut downloader = FakeDownloader::with_delay(std::time::Duration::from_millis(50));
        downloader
            .indices
            .insert(remote_index_path.clone(), create_index_bytes(1).await?);
        let downloader = Arc::new(downloader);
        let barrier = Arc::new(tokio::sync::Barrier::new(N));

        let mut tasks = Vec::with_capacity(N);
        for _ in 0..N {
            let cache = cache.clone();
            let downloader = downloader.clone();
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                cache
                    .find(
                        ARCHIVE_NAME,
                        LATEST_BUILD_ID,
                        FILE_IN_ARCHIVE,
                        downloader.as_ref(),
                    )
                    .await
            }));
        }

        for task in tasks {
            let result = task.await??;
            assert!(result.is_some());
        }
        assert_eq!(downloader.download_count(&remote_index_path), 1);

        Ok(())
    }

    /// Build an index from a set of (path, content) pairs and open it as an `Index`.
    async fn index_from_entries(entries: Vec<(&'static str, &'static [u8])>) -> Result<Index> {
        let archive = create_archive_from_entries(entries).await?;
        let tmp = tempfile::NamedTempFile::new()?.into_temp_path();
        create(archive, &tmp).await?;

        open_archive_index(&tmp).await
    }

    async fn collect_folder_contents(
        index: &mut Index,
        folder: Option<&str>,
    ) -> Result<(Vec<String>, Vec<String>)> {
        let entries: Vec<FolderEntry> = index
            .folder_contents(folder.map(Path::new))
            .try_collect()
            .await?;

        let mut files = Vec::new();
        let mut dirs = Vec::new();
        for entry in entries {
            match entry {
                FolderEntry::File(path, _) => files.push(path),
                FolderEntry::Dir(name) => dirs.push(name),
            }
        }
        files.sort();
        dirs.sort();
        Ok((files, dirs))
    }

    #[tokio::test]
    async fn folder_contents_empty_archive() -> Result<()> {
        let mut index = index_from_entries(vec![]).await?;
        for folder in [None, Some("missing")] {
            let (files, dirs) = collect_folder_contents(&mut index, folder).await?;
            assert!(files.is_empty());
            assert!(dirs.is_empty());
        }
        Ok(())
    }

    #[tokio::test]
    async fn folder_contents_prefix_boundaries_and_literal_names() -> Result<()> {
        let mut index = index_from_entries(vec![
            ("src", b""),
            ("src/\u{10ffff}.rs", b""),
            ("src/\u{10ffff}/nested.rs", b""),
            ("src/Case.rs", b""),
            ("src/case.rs", b""),
            ("src/quote'_%?.rs", b""),
            ("src-other/outside.rs", b""),
            ("src0/outside.rs", b""),
            ("src2/outside.rs", b""),
            ("SRC/outside.rs", b""),
            ("quote'_%?/literal.rs", b""),
            ("quote'ax/outside.rs", b""),
        ])
        .await?;

        let (files, dirs) = collect_folder_contents(&mut index, Some("src")).await?;
        assert_eq!(
            files,
            vec!["Case.rs", "case.rs", "quote'_%?.rs", "\u{10ffff}.rs"]
        );
        assert_eq!(dirs, vec!["\u{10ffff}"]);

        let (files, dirs) = collect_folder_contents(&mut index, Some("quote'_%?")).await?;
        assert_eq!(files, vec!["literal.rs"]);
        assert!(dirs.is_empty());

        let (files, dirs) = collect_folder_contents(&mut index, None).await?;
        assert_eq!(files, vec!["src"]);
        assert_eq!(
            dirs,
            vec![
                "SRC",
                "quote'_%?",
                "quote'ax",
                "src",
                "src-other",
                "src0",
                "src2"
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn folder_contents_unicode_prefix_and_same_named_file_and_dir() -> Result<()> {
        let mut index = index_from_entries(vec![
            ("源/目录/同名", b""),
            ("源/目录/同名/文件.rs", b""),
            ("源/目录/同名/另一个.rs", b""),
            ("源/目录/直接.rs", b""),
            ("源/目录外/忽略.rs", b""),
        ])
        .await?;

        for folder in ["源/目录", "源/目录/"] {
            let (files, dirs) = collect_folder_contents(&mut index, Some(folder)).await?;
            assert_eq!(files, vec!["同名", "直接.rs"]);
            assert_eq!(dirs, vec!["同名"]);
        }
        Ok(())
    }

    #[tokio::test]
    async fn folder_contents_root_lists_files_and_dirs() -> Result<()> {
        let mut index = index_from_entries(vec![
            ("index.html", b""),
            ("style.css", b""),
            ("sub/page.html", b""),
            ("other/file.js", b""),
        ])
        .await?;

        let (files, dirs) = collect_folder_contents(&mut index, None).await?;

        assert_eq!(files, vec!["index.html", "style.css"]);
        assert_eq!(dirs, vec!["other", "sub"]);

        Ok(())
    }

    #[tokio::test]
    async fn folder_contents_subfolder_lists_direct_children_only() -> Result<()> {
        let mut index = index_from_entries(vec![
            ("src/main.rs", b""),
            ("src/lib.rs", b""),
            ("src/utils/helper.rs", b""),
            ("src/utils/mod.rs", b""),
            ("README.md", b""),
        ])
        .await?;

        let (files, dirs) = collect_folder_contents(&mut index, Some("src")).await?;

        assert_eq!(files, vec!["lib.rs", "main.rs"]);
        assert_eq!(dirs, vec!["utils"]);

        Ok(())
    }

    #[tokio::test]
    async fn folder_contents_nested_subfolder() -> Result<()> {
        let mut index = index_from_entries(vec![
            ("a/b/c/deep.txt", b""),
            ("a/b/file.txt", b""),
            ("a/b/other.txt", b""),
        ])
        .await?;

        let (files, dirs) = collect_folder_contents(&mut index, Some("a/b")).await?;

        assert_eq!(files, vec!["file.txt", "other.txt"]);
        assert_eq!(dirs, vec!["c"]);

        Ok(())
    }

    #[tokio::test]
    async fn folder_contents_empty_folder_returns_nothing() -> Result<()> {
        let mut index = index_from_entries(vec![("a/file.txt", b"")]).await?;

        let (files, dirs) = collect_folder_contents(&mut index, Some("nonexistent")).await?;

        assert!(files.is_empty());
        assert!(dirs.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn folder_contents_root_with_only_files() -> Result<()> {
        let mut index = index_from_entries(vec![("a.txt", b""), ("b.txt", b"")]).await?;

        let (files, dirs) = collect_folder_contents(&mut index, None).await?;

        assert_eq!(files, vec!["a.txt", "b.txt"]);
        assert!(dirs.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn folder_contents_subdir_deduplicated() -> Result<()> {
        let mut index = index_from_entries(vec![
            ("sub/a.txt", b""),
            ("sub/b.txt", b""),
            ("sub/c.txt", b""),
        ])
        .await?;

        let (files, dirs) = collect_folder_contents(&mut index, None).await?;

        assert!(files.is_empty());
        // "sub" should appear exactly once despite three files inside it
        assert_eq!(dirs, vec!["sub"]);

        Ok(())
    }

    #[tokio::test]
    async fn folder_contents_treats_glob_chars_literally() -> Result<()> {
        let mut index = index_from_entries(vec![
            ("src[abc]/literal.rs", b""),
            ("srca/wildcard.rs", b""),
            ("srcb/wildcard.rs", b""),
            ("srcc/wildcard.rs", b""),
            ("src*/star.rs", b""),
            ("srcx/star.rs", b""),
            ("src?/question.rs", b""),
            ("srcy/question.rs", b""),
        ])
        .await?;

        let (files, dirs) = collect_folder_contents(&mut index, Some("src[abc]")).await?;
        assert_eq!(files, vec!["literal.rs"]);
        assert!(dirs.is_empty());

        let (files, dirs) = collect_folder_contents(&mut index, Some("src*")).await?;
        assert_eq!(files, vec!["star.rs"]);
        assert!(dirs.is_empty());

        let (files, dirs) = collect_folder_contents(&mut index, Some("src?")).await?;
        assert_eq!(files, vec!["question.rs"]);
        assert!(dirs.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn list_returns_all_entries() -> Result<()> {
        let mut index = index_from_entries(vec![
            ("index.html", b""),
            ("src/main.rs", b""),
            ("src/lib.rs", b""),
        ])
        .await?;

        let mut entries: Vec<FileInfo> = index.list().try_collect().await?;
        entries.sort_by(|a, b| a.path.cmp(&b.path));

        let paths: Vec<&Path> = entries.iter().map(|e| e.path.as_path()).collect();
        assert_eq!(
            paths,
            vec![
                Path::new("index.html"),
                Path::new("src/lib.rs"),
                Path::new("src/main.rs"),
            ]
        );

        Ok(())
    }

    #[tokio::test]
    async fn list_empty_archive() -> Result<()> {
        let mut index = index_from_entries(vec![]).await?;
        let entries: Vec<FileInfo> = index.list().try_collect().await?;
        assert!(entries.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn list_preserves_range_and_compression() -> Result<()> {
        let mut index = index_from_entries(vec![("file.txt", b"hello")]).await?;
        let entries: Vec<FileInfo> = index.list().try_collect().await?;

        assert_eq!(entries.len(), 1);
        // The range should be non-empty and compression should be Bzip2
        // (set by create_archive_from_entries).
        let fi = &entries[0];
        assert!(!fi.range().is_empty());
        assert_eq!(fi.compression(), CompressionAlgorithm::Bzip2);

        Ok(())
    }

    #[tokio::test]
    async fn folder_contents_file_mime_correct() -> Result<()> {
        let mut index = index_from_entries(vec![
            ("main.rs", b""),
            ("README.md", b""),
            ("style.css", b""),
            ("data.json", b""),
            ("index.html", b""),
        ])
        .await?;

        let entries: Vec<FolderEntry> = index.folder_contents(None::<&Path>).try_collect().await?;

        let mut mime_map: Vec<(&str, String)> = entries
            .iter()
            .filter_map(|e| match e {
                FolderEntry::File(name, mime) => Some((name.as_str(), mime.to_string())),
                FolderEntry::Dir(_) => None,
            })
            .collect();
        mime_map.sort_by_key(|(name, _)| *name);

        assert_eq!(
            mime_map,
            vec![
                ("README.md", "text/markdown".to_string()),
                ("data.json", "application/json".to_string()),
                ("index.html", "text/html".to_string()),
                ("main.rs", "text/rust".to_string()),
                ("style.css", "text/css".to_string()),
            ]
        );

        Ok(())
    }

    #[tokio::test]
    async fn folder_contents_dirs_have_no_mime() -> Result<()> {
        let mut index =
            index_from_entries(vec![("src/main.rs", b""), ("docs/readme.md", b"")]).await?;

        let entries: Vec<FolderEntry> = index.folder_contents(None::<&Path>).try_collect().await?;

        for entry in &entries {
            if let FolderEntry::Dir(_) = entry {
                assert!(entry.mime().is_none());
            }
        }

        Ok(())
    }

    /// these are some stress-tests you can use to test if we have any concurrency issues
    /// in our fetch / download / repair logic.
    /// Disabled by default since they might be flaky.
    #[cfg(test)]
    #[cfg(feature = "stress-tests")]
    mod stress_tests {
        use super::*;
        use std::sync::Arc;
        use test_case::test_case;
        use tokio::{sync, task, time};

        const CONCURRENT_ARCHIVE: &str = "concurrent.zip";

        struct ConcurrentArchive {
            downloader: FakeDownloader,
            expected: Vec<FileInfo>,
        }

        async fn concurrent_archive_fixture() -> Result<Arc<ConcurrentArchive>> {
            let path = create_index(2).await?;

            let mut index = open_archive_index(&path).await?;

            let expected: Vec<_> = index.list().try_collect().await?;

            let mut downloader = FakeDownloader::new();
            downloader.indices.insert(
                format!("{CONCURRENT_ARCHIVE}.{ARCHIVE_INDEX_FILE_EXTENSION}"),
                fs::read(&path).await?,
            );
            Ok(Arc::new(ConcurrentArchive {
                downloader,
                expected,
            }))
        }

        async fn check_archive_request(
            cache: &Cache,
            fixture: &ConcurrentArchive,
            request: usize,
        ) -> Result<()> {
            let expected = &fixture.expected[request % fixture.expected.len()];
            let path = expected.path.to_str().unwrap();
            // Exercise both the lookup retry loop and an index returned to a caller.
            let actual = if request % 3 == 2 {
                cache
                    .find_index(CONCURRENT_ARCHIVE, None, &fixture.downloader)
                    .await?
                    .find(path)
                    .await?
            } else {
                cache
                    .find(CONCURRENT_ARCHIVE, None, path, &fixture.downloader)
                    .await?
            };
            anyhow::ensure!(
                actual.as_ref() == Some(expected),
                "wrong index metadata for {path}: expected {expected:?}, got {actual:?}"
            );
            Ok(())
        }

        /// Stress-test the archive index cache using real SQLite files.
        ///
        /// 64 parallel readers, each calls `rounds` cache lookups
        /// (`check_archive_request`).
        ///
        /// runs a "breaker" after each round, that can break things,
        /// that the index should repair / handle in the next round.
        async fn concurrent_archive_requests<F, Fut>(
            cache: Arc<TestEnv>,
            fixture: Arc<ConcurrentArchive>,
            rounds: usize,
            breaker: F,
        ) -> Result<()>
        where
            Fut: Future<Output = Result<()>> + Send + 'static,
            F: Fn(Arc<TestEnv>) -> Fut + Send + 'static,
        {
            const READERS: usize = 64;
            let barrier = Arc::new(sync::Barrier::new(READERS + 1));
            let mut tasks = task::JoinSet::new();
            for reader in 0..READERS {
                let cache = cache.clone();
                let fixture = fixture.clone();
                let barrier = barrier.clone();
                tasks.spawn(async move {
                    let mut errors = Vec::new();
                    for round in 0..rounds {
                        barrier.wait().await;
                        // Keep participating after errors so all rounds complete and
                        // a failed request cannot strand other tasks at the barrier.
                        if let Err(err) =
                            check_archive_request(&cache, &fixture, reader + round).await
                        {
                            errors.push(format!("reader {reader}, round {round}: {err:#}"));
                        }
                    }
                    errors
                });
            }

            // this spawns the "breaker" that breaks things after each round.
            tasks.spawn(async move {
                let mut errors = Vec::new();
                for round in 0..rounds {
                    barrier.wait().await;
                    if let Err(err) = breaker(cache.clone()).await {
                        errors.push(format!("breaker, round {round}: {err:#}"));
                    }
                }
                errors
            });

            let errors = time::timeout(Duration::from_secs(60), async {
                let mut errors = Vec::new();
                while let Some(result) = tasks.join_next().await {
                    errors.extend(result?);
                }
                anyhow::Ok(errors)
            })
            .await
            .context("concurrent archive requests timed out")??;

            anyhow::ensure!(
                errors.is_empty(),
                "{} failures during concurrent archive requests (first 10):\n{}",
                errors.len(),
                errors
                    .iter()
                    .take(10)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            Ok(())
        }

        #[tokio::test(flavor = "multi_thread")]
        #[test_case("cold")]
        #[test_case("missing")]
        #[test_case("corrupt")]
        async fn concurrent_archive_repair(state: &'static str) -> Result<()> {
            let cache = Arc::new(test_cache().await?);
            let fixture = concurrent_archive_fixture().await?;
            let local_index = cache.local_index_path(CONCURRENT_ARCHIVE, None);

            // Each round releases 64 readers and a "breaker" together. This probes the
            // remaining filesystem races; success is not proof that they are absent.
            //
            // We run 64 rounds.
            concurrent_archive_requests(cache, fixture, 64, move |cache| {
                let local_index = local_index.clone();

                async move {
                    match state {
                        "cold" => cache.purge(CONCURRENT_ARCHIVE, None).await?,
                        "missing" => {
                            if fs::try_exists(&local_index).await? {
                                fs::remove_file(&local_index).await?
                            }
                        }
                        "corrupt" => {
                            let corrupt = local_index.with_extension("corrupt");
                            fs::write(&corrupt, CORRUPT_SQLITE_INDEX).await?;
                            fs::rename(corrupt, &local_index).await?;
                        }
                        _ => unreachable!(),
                    };
                    anyhow::Ok(())
                }
            })
            .await
            .with_context(|| format!("{state} local archive index"))?;

            Ok(())
        }

        #[tokio::test]
        async fn broken_indexes() -> Result<()> {
            let temp_path = tempfile::NamedTempFile::new()?.into_temp_path();
            fs::write(&temp_path, CORRUPT_SQLITE_INDEX).await?;

            assert!(open_archive_index(&temp_path).await.is_err());

            Ok(())
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn concurrent_find_triggers_single_download_per_index() -> Result<()> {
            let cache = test_cache().await?;
            let cache = Arc::new(cache);
            const N: usize = 16;
            const LATEST_BUILD_ID: Option<BuildId> = Some(BuildId(7));
            const ARCHIVE_NAME: &str = "test.zip";
            const FILE_IN_ARCHIVE: &str = "testfile0";

            let remote_index_path = format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}");
            let mut downloader = FakeDownloader::with_delay(std::time::Duration::from_millis(50));
            downloader
                .indices
                .insert(remote_index_path.clone(), create_index_bytes(1).await?);
            let downloader = Arc::new(downloader);
            let barrier = Arc::new(tokio::sync::Barrier::new(N));

            let mut tasks = Vec::with_capacity(N);
            for _ in 0..N {
                let cache = cache.clone();
                let downloader = downloader.clone();
                let barrier = barrier.clone();
                tasks.push(tokio::spawn(async move {
                    barrier.wait().await;
                    cache
                        .find(
                            ARCHIVE_NAME,
                            LATEST_BUILD_ID,
                            FILE_IN_ARCHIVE,
                            downloader.as_ref(),
                        )
                        .await
                }));
            }

            for task in tasks {
                let result = task.await??;
                assert!(result.is_some());
            }
            assert_eq!(downloader.download_count(&remote_index_path), 1);

            Ok(())
        }
    }
}
