use crate::{
    PathNotFoundError, blob::StreamingBlob, config::ArchiveIndexCacheConfig, file::FolderEntry,
    types::FileRange, utils::file_list::walk_dir_recursive,
};
use anyhow::{Context as _, Result, anyhow, bail};
use async_stream::try_stream;
use docs_rs_mimes::detect_mime;
use docs_rs_opentelemetry::AnyMeterProvider;
use docs_rs_types::{BuildId, CompressionAlgorithm};
use docs_rs_utils::spawn_blocking;
use futures_util::{Stream, TryStreamExt as _};
use moka::future::Cache as MokaCache;
use opentelemetry::{
    KeyValue,
    metrics::{Counter, Gauge, Histogram},
};
use sqlx::{ConnectOptions as _, Connection as _, QueryBuilder, Row as _, Sqlite};
use std::{
    collections::HashSet,
    fmt,
    future::Future,
    path::{Path, PathBuf},
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
    sync::mpsc,
    task::JoinHandle,
};
use tokio_util::io::SyncIoBridge;
use tracing::{debug, error, info, instrument, trace, warn};

pub(crate) const ARCHIVE_INDEX_FILE_EXTENSION: &str = "index";

/// dummy size we assume in case of errors
const DUMMY_FILE_SIZE: u64 = 1024 * 1024; // 1 MiB
/// self-repair attempts
const FIND_ATTEMPTS: usize = 5;

#[derive(Debug)]
struct Metrics {
    // calls to find an entry in the local cache
    find_calls: Counter<u64>,

    // local cache eviction
    evicted_entries: Counter<u64>,
    evicted_bytes_total: Counter<u64>,
    evicted_entry_size: Histogram<u64>,

    // local cache misses / downloads & bytes
    // includes & doesn't differentiate retries / repairs for now
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
        const KIB: f64 = 1024.0;
        const MIB: f64 = 1024.0 * KIB;
        const GIB: f64 = 1024.0 * MIB;

        let entry_size_boundaries = vec![
            500.0 * KIB,
            1.0 * MIB,
            2.0 * MIB,
            4.0 * MIB,
            8.0 * MIB,
            16.0 * MIB,
            32.0 * MIB,
            64.0 * MIB,
            128.0 * MIB,
            256.0 * MIB,
            512.0 * MIB,
            1.0 * GIB,
            2.0 * GIB,
            4.0 * GIB,
            8.0 * GIB,
            10.0 * GIB,
        ];

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
}

#[derive(PartialEq, Eq, Debug)]
pub struct FileInfo {
    path: PathBuf,
    range: FileRange,
    compression: CompressionAlgorithm,
}

pub(crate) struct Entry {
    // file size of the local sqlite database.
    // Will be used to "weigh" cache entries, so that the cache can evict based on
    // total size of cached files instead of number of entries.
    file_size_kib: u32,
}

impl Entry {
    fn from_size(file_size: u64) -> Self {
        let file_size_kib = file_size.div_ceil(1024).max(1).min(u32::MAX as u64) as u32;
        Self { file_size_kib }
    }

    async fn from_path(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        Self::from_size(match fs::metadata(&path).await {
            Ok(meta) => meta.len(),
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
    /// on the local files that are already.
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

        let metrics = Arc::new(Metrics::new(meter_provider));
        let metrics_for_eviction = metrics.clone();
        let manager = CacheManager::builder()
            .initial_capacity(config.expected_count)
            // Time to idle (TTI): A cached entry will be expired after
            // the specified duration past from get or insert.
            // We don't set TTL (time to live), which would be just time-after-insert.
            .time_to_idle(config.ttl)
            // we weigh each cache entry by the file size of the sqlite database.
            // The max size of the cache for all of docs.rs is 500 GiB at the time of writing.
            // In KiB, this would be around 500k, which makes KiB the right unit.
            // Anything bigger (like MiB) would mean that we count smaller dbs than 1 MiB as if
            // they were 1 MiB big.
            .weigher(|_key: &PathBuf, entry: &Arc<Entry>| -> u32 { entry.file_size_kib })
            // max capacity
            // not entries, but _weighted entries_.
            // with the weight fn from above, the max capacity is a storage size value.
            .max_capacity(config.max_size_mb * 1024)
            // the eviction listener is called when moka evicts a cache entry.
            // In this case we want to delete the corresponding local files.
            .eviction_listener(move |path, entry, reason| {
                let path = path.to_path_buf();
                let metrics = metrics_for_eviction.clone();
                // The spawned task means file deletion is deferred. See the
                // "benign race with the eviction listener" comment in `find_inner`
                // for why this is acceptable.
                tokio::spawn(async move {
                    let reason = format!("{reason:?}");
                    let evicted_bytes = entry.file_size_kib as u64 * 1024;
                    let reason_attr = [KeyValue::new("cause", reason.clone())];

                    metrics.evicted_entries.add(1, &reason_attr);
                    metrics.evicted_bytes_total.add(evicted_bytes, &reason_attr);
                    metrics
                        .evicted_entry_size
                        .record(evicted_bytes, &reason_attr);

                    trace!(
                        ?path,
                        ?reason_attr,
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
                });
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
    /// While this is running, our `find_inner` & `download_archive_index` logic will just
    /// fill it itself.
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
                                Arc::new(Entry::from_size(item.metadata.len()))
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

    async fn remove_local_index(path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        for ext in &["wal", "shm"] {
            let to_delete = path.with_extension(format!("{ARCHIVE_INDEX_FILE_EXTENSION}-{ext}"));
            let _ = fs::remove_file(&to_delete).await;
        }

        if let Err(err) = fs::remove_file(&path).await
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

    pub(crate) async fn find_index(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        downloader: &impl Downloader,
    ) -> Result<Index> {
        let local_index_path = self.local_index_path(archive_path, latest_build_id);

        // fast path: try to use whatever is there, no locking
        let force_redownload = match Index::open(&local_index_path).await {
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
                let force_redownload = !err.is::<PathNotFoundError>();
                debug!(?err, "archive index open failed, will try repair.");
                force_redownload
            }
        };

        let remote_index_path = format!("{archive_path}.{ARCHIVE_INDEX_FILE_EXTENSION}");

        // moka will coalesce all concurrent calls to try_get_with_by_ref with the same key
        // into a single call to the async closure.
        // https://docs.rs/moka/0.12.14/moka/future/struct.Cache.html#concurrent-calls-on-the-same-key
        // So we don't need any locking here to prevent multiple downloads for the same
        // missing archive index.
        self.manager
            .try_get_with_by_ref(&local_index_path, async {
                // NOTE: benign race with the eviction listener.
                //
                // When moka evicts an entry (time/size pressure), it removes it from the
                // cache immediately but runs the eviction listener later (via a spawned
                // tokio task that deletes the local file).
                //
                // If a new request arrives between the cache removal and the file deletion:
                //   1. Cache miss → we enter this closure.
                //   2. `try_exists` → true (file not deleted yet).
                //   3. We re-insert the existing file into the cache.
                //   4. The eviction listener's spawned task then runs and deletes the file
                //      out from under us.
                //   5. The next `find` call fails on the fast path (file gone), falls back
                //      into this closure, sees `try_exists` → false, and re-downloads.
                //
                // Net impact: one request pays the cost of an extra S3 download. No error
                // is visible to the user since the self-repair logic handles it.
                let entry = if !force_redownload && fs::try_exists(&local_index_path).await? {
                    // after server startup we might have local indexes that don't
                    // yet exist in our cache manager.
                    // So we only need to download if the file doesn't exist.
                    Entry::from_path(&local_index_path).await
                } else {
                    if force_redownload {
                        Self::remove_local_index(&local_index_path).await?;
                    }
                    Entry::from_size(
                        self.download_archive_index(
                            downloader,
                            &local_index_path,
                            &remote_index_path,
                        )
                        .await?,
                    )
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

        // Final attempt: if this still fails, bubble the error.
        Index::open(local_index_path).await
    }

    /// Find the file metadata needed to fetch a certain path inside a remote archive.
    /// Will try to use a local cache of the index file, and otherwise download it
    /// from storage.
    #[instrument(skip(self, downloader))]
    pub(crate) async fn find(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        path_in_archive: &str,
        downloader: &impl Downloader,
    ) -> Result<Option<FileInfo>> {
        for attempt in 1..=FIND_ATTEMPTS {
            let result = async {
                let mut index = self
                    .find_index(archive_path, latest_build_id, downloader)
                    .await?;
                index.find(path_in_archive).await
            }
            .await;

            match result {
                Ok(file_info) => {
                    self.metrics.find_calls.add(
                        1,
                        &[
                            KeyValue::new("attempt", attempt.to_string()),
                            KeyValue::new("outcome", "success"),
                        ],
                    );
                    return Ok(file_info);
                }
                Err(err) if attempt < FIND_ATTEMPTS => {
                    warn!(
                        ?err,
                        %attempt,
                        "error in archive index lookup, purging local cache and retrying"
                    );
                    self.purge(archive_path, latest_build_id).await?;
                }
                Err(err) => {
                    self.metrics.find_calls.add(
                        1,
                        &[
                            KeyValue::new("attempt", attempt.to_string()),
                            KeyValue::new("outcome", "error"),
                        ],
                    );
                    return Err(err);
                }
            }
        }

        unreachable!("find retry loop exited unexpectedly");
    }

    #[instrument(skip(self, downloader))]
    pub(crate) async fn download_archive_index(
        &self,
        downloader: &impl Downloader,
        local_index_path: &Path,
        remote_index_path: &str,
    ) -> Result<u64> {
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

        self.metrics.downloads.add(1, &[]);
        self.metrics.downloaded_bytes.add(copied, &[]);
        self.metrics.downloaded_entry_size.record(copied, &[]);

        Ok(copied)
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

/// creates a new empty SQLite database, and returns a configured connection
/// pool to connect to the DB.
/// Any existing DB at the given path will be deleted first.
async fn sqlite_create<P: AsRef<Path>>(path: P) -> Result<sqlx::SqliteConnection> {
    let path = path.as_ref();
    if fs::try_exists(&path).await? {
        fs::remove_file(path).await?;
    }

    sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .read_only(false)
        .pragma("synchronous", "full")
        .create_if_missing(true)
        .connect()
        .await
        .map_err(Into::into)
}

/// open existing SQLite database, return a configured connection poll
/// to connect to the DB.
/// Will error when the database doesn't exist at that path.
async fn sqlite_open<P: AsRef<Path>>(path: P) -> Result<sqlx::SqliteConnection> {
    sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .immutable(true)
        .pragma("synchronous", "off") // not needed for readonly db
        .pragma("temp_store", "MEMORY")
        .pragma("query_only", "ON")
        .pragma("mmap_size", "536870912") // 512 MiB
        .pragma("cache_size", "-4096") // 4 MiB
        .serialized(false) // same as OPEN_NOMUTEX
        .create_if_missing(false)
        .connect()
        .await
        .map_err(Into::into)
}

/// create an archive index based on a zipfile.
///
/// Will delete the destination file if it already exists.
#[instrument(skip(zipfile))]
pub(crate) async fn create<R, P>(zipfile: R, destination: P) -> Result<R>
where
    R: AsyncRead + AsyncSeek + Unpin + Send + 'static,
    P: AsRef<Path> + std::fmt::Debug,
{
    let mut conn = sqlite_create(destination).await?;
    let mut tx = conn.begin().await?;

    sqlx::query(
        r#"
            CREATE TABLE files (
                id INTEGER PRIMARY KEY,
                path TEXT UNIQUE,
                start INTEGER,
                end INTEGER,
                compression INTEGER
            );
        "#,
    )
    .execute(&mut *tx)
    .await?;

    let (tx_entries, mut rx_entries) = mpsc::channel::<(String, u64, u64, i32)>(1000);

    let zip_task = spawn_blocking(move || {
        let mut bridge = SyncIoBridge::new(zipfile);
        let mut archive = zip::ZipArchive::new(&mut bridge)?;
        for i in 0..archive.len() {
            let entry = archive.by_index(i)?;

            let start = entry
                .data_start()
                .ok_or_else(|| anyhow!("missing data_start in zip directory"))?;
            let end = start + entry.compressed_size() - 1;
            let compression_raw = match entry.compression() {
                zip::CompressionMethod::Bzip2 => CompressionAlgorithm::Bzip2 as i32,
                zip::CompressionMethod::Deflated => CompressionAlgorithm::Deflate as i32,
                c => bail!("unsupported compression algorithm {} in zip-file", c),
            };

            tx_entries
                .blocking_send((entry.name().to_string(), start, end, compression_raw))
                .map_err(|_| anyhow!("archive index receiver dropped"))?;
        }
        drop(archive);
        Ok(bridge.into_inner())
    });

    const CHUNKS: usize = 1000;
    let mut chunk = Vec::with_capacity(CHUNKS);
    loop {
        let received = rx_entries.recv_many(&mut chunk, CHUNKS).await;
        if received == 0 {
            break;
        }
        let mut insert_stmt =
            QueryBuilder::<Sqlite>::new("INSERT INTO files (path, start, end, compression) ");
        insert_stmt.push_values(
            chunk.drain(..),
            |mut b, (path, start, end, compression_raw)| {
                b.push_bind(path)
                    .push_bind(start as i64)
                    .push_bind(end as i64)
                    .push_bind(compression_raw);
            },
        );
        insert_stmt
            .build()
            .persistent(false)
            .execute(&mut *tx)
            .await?;
    }

    let zipfile = zip_task.await?;

    sqlx::query("CREATE INDEX idx_files_path ON files (path);")
        .execute(&mut *tx)
        .await?;

    // Commit the transaction before VACUUM (VACUUM cannot run inside a transaction)
    tx.commit().await?;

    // VACUUM outside the transaction
    sqlx::query("VACUUM").execute(&mut conn).await?;

    Ok(zipfile)
}

pub struct Index {
    conn: sqlx::SqliteConnection,
}

impl Index {
    pub(crate) async fn open<P>(archive_index_path: P) -> Result<Self>
    where
        P: AsRef<Path>,
    {
        let archive_index_path = archive_index_path.as_ref().to_path_buf();
        let conn = sqlite_open(&archive_index_path).await?;
        Ok(Self { conn })
    }

    #[instrument(skip(self))]
    pub async fn find<P>(&mut self, search_for: P) -> Result<Option<FileInfo>>
    where
        P: AsRef<Path> + fmt::Debug,
    {
        let search_for = search_for.as_ref();

        if search_for.is_absolute() {
            bail!("search path in archive index has to be relative");
        }

        let search_str = search_for
            .to_str()
            .ok_or_else(|| anyhow!("non-UTF-8 path in archive index lookup"))?;

        // now actually find the entry in the index
        let row = sqlx::query(
            "SELECT start, end, compression
                FROM files
                WHERE path = ?",
        )
        .bind(search_str)
        .fetch_optional(&mut self.conn)
        .await
        .context("error fetching SQLite data")?;

        let file_info = if let Some(row) = row {
            let start: u64 = row.try_get(0)?;
            let end: u64 = row.try_get(1)?;
            let compression_raw: i32 = row.try_get(2)?;

            Some(FileInfo {
                path: search_for.to_path_buf(),
                range: start..=end,
                compression: compression_raw.try_into().map_err(|value| {
                    anyhow!("invalid compression algorithm '{value}' in database")
                })?,
            })
        } else {
            None
        };

        Ok(file_info)
    }

    pub fn list(&mut self) -> impl Stream<Item = Result<FileInfo>> + '_ {
        try_stream! {
            let mut rows = sqlx::query(
                "SELECT path, start, end, compression FROM files"
            )
            .fetch(&mut self.conn);

            while let Some(row) = rows.try_next().await.context("error fetching SQLite data")? {
                let path: String = row.try_get(0)?;
                let start: u64 = row.try_get(1)?;
                let end: u64 = row.try_get(2)?;
                let compression_raw: i32 = row.try_get(3)?;
                let path = PathBuf::from(path);
                debug_assert!(path.is_relative());

                yield FileInfo {
                    path,
                    range: start..=end,
                    compression: compression_raw.try_into().map_err(|value| {
                        anyhow!("invalid compression algorithm '{value}' in database")
                    })?,
                };
            }
        }
    }

    /// get the folder contents inside the zip archive.
    /// * missing folder = list the root
    /// * given folder: just lists the files in there, and subfolders, but not their contents.
    ///
    /// You'll need this method when you build a file-browser for the archive, like
    /// in our source pages.
    #[instrument(skip(self))]
    pub fn folder_contents<P>(
        &mut self,
        folder: Option<P>,
    ) -> impl Stream<Item = Result<FolderEntry>> + '_
    where
        P: AsRef<Path> + std::fmt::Debug,
    {
        // Build the path prefix string used in GLOB patterns.
        // For root (None): prefix = ""
        // For a folder:    prefix = "some/folder/"
        let prefix: Option<String> = folder.as_ref().map(|f| {
            let s = f.as_ref().to_string_lossy();
            // Normalize: strip any trailing slash, then re-add exactly one.
            format!("{}/", s.trim_end_matches('/'))
        });

        try_stream! {
            // Seen-dirs is the only state we must accumulate: one String per unique
            // immediate subdirectory name. File rows are yielded as they arrive.
            let mut seen_dirs: HashSet<String> = HashSet::new();


            let mut rows = if let Some(prefix) = &prefix {
                let prefix_upper_bound = format!("{prefix}\u{10ffff}");

                // NOTE: we're using >= and < for the prefix matching here.
                // Using `GLOB` would mean we have to escape the path.
                // Other techniques like sqlite string functions would mean the index on the
                // table can't be used.

                sqlx::query("SELECT path FROM files WHERE path >= ? AND path < ?")
                    .bind(prefix)
                    .bind(prefix_upper_bound)
                    .fetch(&mut self.conn)
            } else {
                sqlx::query("SELECT path FROM files")
                    .fetch(&mut self.conn)
            };

            while let Some(row) = rows.try_next().await.context("error fetching entries from SQLite")? {
                let full_path: String = row.try_get(0)?;
                // The relative part is everything after the prefix.
                let rel = if let Some(prefix) = &prefix {
                    // Archive paths are stored as UTF-8 strings, and `full_path` comes from
                    // the same prefix string used in the range query above.
                    debug_assert!(full_path.is_char_boundary(prefix.len()));
                    &full_path[prefix.len()..]
                } else {
                    &full_path
                };

                if let Some(slash_pos) = rel.find('/') {
                    // It's inside a subdirectory. Extract and deduplicate the first component.
                    let dir_name = &rel[..slash_pos];
                    if seen_dirs.insert(dir_name.to_string()) {
                        yield FolderEntry::Dir(dir_name.to_string());
                    }
                } else {
                    // Direct file — yield only the name relative to the queried folder.
                    let rel = rel.to_string();
                    let mime = detect_mime(&rel);
                    yield FolderEntry::File(rel, mime);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{blob::StreamingBlob, storage::non_blocking::ZIP_BUFFER_SIZE};
    use chrono::Utc;
    use docs_rs_config::AppConfig as _;
    use docs_rs_opentelemetry::testing::TestMetrics;
    use sqlx::error::DatabaseError as _;
    use std::{collections::HashMap, io::Cursor, ops::Deref, pin::Pin, sync::Arc};
    use zip::write::SimpleFileOptions;

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
                    content_length: content.len(),
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
                    content_length: content.len(),
                    content: Box::new(Cursor::new(content)),
                })
            })
        }
    }

    async fn create_index_bytes(file_count: u32) -> Result<Vec<u8>> {
        let tf = create_test_archive(file_count).await?;
        let tempfile = tempfile::NamedTempFile::new()?.into_temp_path();
        create(tf, &tempfile).await?;
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

        let mut index = Index::open(&tempfile).await?;
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

        let mut index = Index::open(&tempfile).await?;
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

        let mut conn = sqlite_open(&tempfile).await?;

        let row = sqlx::query("SELECT count(*) FROM files")
            .fetch_one(&mut conn)
            .await?;

        assert_eq!(row.get::<i64, _>(0), 0);

        Ok(())
    }

    #[tokio::test]
    async fn archive_with_more_than_65k_files() -> Result<()> {
        let tf = create_test_archive(100_000).await?;

        let tempfile = tempfile::NamedTempFile::new()?.into_temp_path();
        create(tf, &tempfile).await?;

        let mut conn = sqlite_open(&tempfile).await?;

        let row = sqlx::query("SELECT count(*) FROM files")
            .fetch_one(&mut conn)
            .await?;

        assert_eq!(row.get::<i64, _>(0), 100_000);

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
        fs::write(&cache_file, b"not-an-sqlite-index").await?;
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
    async fn find_retries_once_then_errors() -> Result<()> {
        let cache = test_cache().await?;
        const LATEST_BUILD_ID: Option<BuildId> = Some(BuildId(7));
        const ARCHIVE_NAME: &str = "test.zip";

        let remote_index_path = format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}");
        let mut downloader = FakeDownloader::new();
        downloader
            .indices
            .insert(remote_index_path.clone(), b"not-a-sqlite-index".to_vec());

        let err = cache
            .find(ARCHIVE_NAME, LATEST_BUILD_ID, "testfile0", &downloader)
            .await
            .unwrap_err();

        assert_eq!(
            err.downcast::<sqlx::Error>()
                .unwrap()
                .into_database_error()
                .unwrap()
                .as_error()
                .downcast_ref::<sqlx::sqlite::SqliteError>()
                .unwrap()
                .message(),
            "file is not a database"
        );
        assert_eq!(downloader.download_count(&remote_index_path), FIND_ATTEMPTS);

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
        fs::write(&cache_file, b"not-an-sqlite-index").await?;

        let remote_index_path = format!("{ARCHIVE_NAME}.{ARCHIVE_INDEX_FILE_EXTENSION}");
        let downloader = FlakyDownloader::new(
            remote_index_path,
            create_index_bytes(1).await?,
            FIND_ATTEMPTS - 1,
        );

        let result = cache
            .find(ARCHIVE_NAME, LATEST_BUILD_ID, FILE_IN_ARCHIVE, &downloader)
            .await?;
        assert!(result.is_some());
        assert_eq!(downloader.fetch_count(), FIND_ATTEMPTS);

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

    #[tokio::test(flavor = "multi_thread")]
    async fn manager_invalidate_removes_index_wal_and_shm_via_eviction_listener() -> Result<()> {
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
            .insert(local_index.clone(), Arc::new(Entry::from_size(5)))
            .await;

        cache.manager.invalidate(&local_index).await;
        cache.flush().await?;
        // The eviction listener deletes files in a spawned task;
        // give it time to complete on the multi-thread runtime.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

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

        Index::open(&tmp).await
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
}
