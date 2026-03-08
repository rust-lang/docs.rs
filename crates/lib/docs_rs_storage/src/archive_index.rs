use crate::{Config, blob::StreamingBlob, types::FileRange};
use anyhow::{Context as _, Result, anyhow, bail};
use dashmap::DashMap;
use docs_rs_types::{BuildId, CompressionAlgorithm};
use docs_rs_utils::spawn_blocking;
use sqlx::{ConnectOptions as _, Connection as _, QueryBuilder, Row as _, Sqlite};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};
use tokio::{
    fs,
    io::{self, AsyncRead, AsyncSeek, AsyncWriteExt as _},
    sync::{Mutex, mpsc},
};
use tokio_util::io::SyncIoBridge;
use tracing::{debug, instrument, warn};

pub(crate) const ARCHIVE_INDEX_FILE_EXTENSION: &str = "index";

#[derive(PartialEq, Eq, Debug)]
pub(crate) struct FileInfo {
    range: FileRange,
    compression: CompressionAlgorithm,
}

pub(crate) struct Cache {
    local_archive_cache_path: PathBuf,
    /// Locks to synchronize write-access to the locally cached archive index files.
    locks: DashMap<PathBuf, Arc<Mutex<()>>>,
}

pub(crate) trait Downloader {
    fn fetch_archive_index<'a>(
        &'a self,
        remote_index_path: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<StreamingBlob>> + Send + 'a>>;
}

impl Cache {
    pub(crate) fn new(config: &Config) -> Self {
        Self {
            local_archive_cache_path: config.local_archive_cache_path.clone(),
            locks: DashMap::with_capacity(config.local_archive_cache_expected_count),
        }
    }

    fn local_index_path(&self, archive_path: &str, latest_build_id: Option<BuildId>) -> PathBuf {
        self.local_archive_cache_path.join(format!(
            "{archive_path}.{}.{ARCHIVE_INDEX_FILE_EXTENSION}",
            latest_build_id.map(|id| id.0).unwrap_or(0)
        ))
    }

    fn local_index_cache_lock(&self, local_index_path: impl AsRef<Path>) -> Arc<Mutex<()>> {
        let local_index_path = local_index_path.as_ref().to_path_buf();

        self.locks
            .entry(local_index_path)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .downgrade()
            .clone()
    }

    /// purge a single archive index file
    pub(crate) async fn purge(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
    ) -> Result<()> {
        let local_index_path = self.local_index_path(archive_path, latest_build_id);
        let rwlock = self.local_index_cache_lock(&local_index_path);
        let _write_guard = rwlock.lock().await;

        for ext in &["wal", "shm"] {
            let to_delete =
                local_index_path.with_extension(format!("{ARCHIVE_INDEX_FILE_EXTENSION}-{ext}"));
            let _ = fs::remove_file(&to_delete).await;
        }

        if fs::try_exists(&local_index_path).await? {
            fs::remove_file(&local_index_path).await?;
        }

        Ok(())
    }

    async fn find_inner(
        &self,
        archive_path: &str,
        latest_build_id: Option<BuildId>,
        path_in_archive: &str,
        downloader: &impl Downloader,
    ) -> Result<Option<FileInfo>> {
        let local_index_path = self.local_index_path(archive_path, latest_build_id);

        // fast path: try to use whatever is there, no locking
        match find_in_file(&local_index_path, path_in_archive).await {
            Ok(res) => return Ok(res),
            Err(err) => {
                debug!(?err, "archive index lookup failed, will try repair.");
            }
        }

        let lock = self.local_index_cache_lock(&local_index_path);
        let write_guard = lock.lock().await;

        // Double-check: maybe someone fixed it between our first failure and now.
        if let Ok(res) = find_in_file(&local_index_path, path_in_archive).await {
            return Ok(res);
        }

        let remote_index_path = format!("{archive_path}.{ARCHIVE_INDEX_FILE_EXTENSION}");

        // We are the repairer: download fresh index into place.
        self.download_archive_index(downloader, &local_index_path, &remote_index_path)
            .await?;

        // Write lock is dropped here (end of scope), so others can proceed.
        drop(write_guard);

        // Final attempt: if this still fails, bubble the error.
        find_in_file(local_index_path, path_in_archive).await
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
        for attempt in 0..2 {
            match self
                .find_inner(archive_path, latest_build_id, path_in_archive, downloader)
                .await
            {
                Ok(file_info) => return Ok(file_info),
                Err(err) if attempt == 0 => {
                    warn!(
                        ?err,
                        "error resolving archive index, purging local cache and retrying once"
                    );
                    self.purge(archive_path, latest_build_id).await?;
                }
                Err(err) => return Err(err),
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
    ) -> Result<()> {
        let parent = local_index_path
            .parent()
            .ok_or_else(|| anyhow!("index path without parent"))?
            .to_path_buf();
        fs::create_dir_all(&parent).await?;

        // Create a unique temp file in the cache folder.
        let (temp_file, mut temp_path) = spawn_blocking({
            let folder = self.local_archive_cache_path.clone();
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
        io::copy(&mut stream, &mut temp_file).await?;
        temp_file.flush().await?;
        temp_path.disable_cleanup(true);

        // Publish atomically.
        // Will replace any existing file.
        fs::rename(&temp_path, local_index_path).await?;

        Ok(())
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

/// crates a new empty SQLite database, and returns a configured connection
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

    let compression_bzip = CompressionAlgorithm::Bzip2 as i32;
    let (tx_entries, mut rx_entries) = mpsc::channel::<(String, u64, u64, i32)>(1000);

    let zip_task = spawn_blocking(move || {
        let mut bridge = SyncIoBridge::new(zipfile);
        let mut archive = zip::ZipArchive::new(&mut bridge)?;
        for i in 0..archive.len() {
            let entry = archive.by_index(i)?;

            let start = entry
                .data_start()
                .ok_or_else(|| anyhow!("missing data_start in zip derectory"))?;
            let end = start + entry.compressed_size() - 1;
            let compression_raw = match entry.compression() {
                zip::CompressionMethod::Bzip2 => compression_bzip,
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

async fn find_in_sqlite_index<'e, E>(executor: E, search_for: &str) -> Result<Option<FileInfo>>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let row = sqlx::query(
        "
        SELECT start, end, compression
        FROM files
        WHERE path = ?
        ",
    )
    .bind(search_for)
    .fetch_optional(executor)
    .await
    .context("error fetching SQLite data")?;

    if let Some(row) = row {
        let start: u64 = row.try_get(0)?;
        let end: u64 = row.try_get(1)?;
        let compression_raw: i32 = row.try_get(2)?;

        Ok(Some(FileInfo {
            range: start..=end,
            compression: compression_raw.try_into().map_err(|value| {
                anyhow::anyhow!(format!(
                    "invalid compression algorithm '{value}' in database"
                ))
            })?,
        }))
    } else {
        Ok(None)
    }
}

#[instrument]
pub(crate) async fn find_in_file<P>(
    archive_index_path: P,
    search_for: &str,
) -> Result<Option<FileInfo>>
where
    P: AsRef<Path> + std::fmt::Debug,
{
    let mut conn = sqlite_open(archive_index_path).await?;

    find_in_sqlite_index(&mut conn, search_for).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, blob::StreamingBlob, types::StorageKind};
    use chrono::Utc;
    use sqlx::error::DatabaseError as _;
    use std::{collections::HashMap, io::Cursor, ops::Deref, pin::Pin, sync::Arc};
    use zip::write::SimpleFileOptions;

    async fn create_test_archive(file_count: u32) -> Result<fs::File> {
        spawn_blocking(move || {
            use std::io::Write as _;

            let tf = tempfile::tempfile()?;

            let objectcontent: Vec<u8> = (0..255).collect();

            let mut archive = zip::ZipWriter::new(tf);
            for i in 0..file_count {
                archive.start_file(
                    format!("testfile{i}"),
                    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Bzip2),
                )?;
                archive.write_all(&objectcontent)?;
            }
            Ok(archive.finish()?)
        })
        .await
        .map(fs::File::from_std)
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

    async fn create_index_bytes(file_count: u32) -> Result<Vec<u8>> {
        let tf = create_test_archive(file_count).await?;
        let tempfile = tempfile::NamedTempFile::new()?.into_temp_path();
        create(tf, &tempfile).await?;
        fs::read(&tempfile).await.map_err(Into::into)
    }

    struct TestEnv {
        _cache_dir: tempfile::TempDir,
        _config: Config,
        cache: Cache,
    }

    impl Deref for TestEnv {
        type Target = Cache;

        fn deref(&self) -> &Self::Target {
            &self.cache
        }
    }

    fn test_cache() -> Result<TestEnv> {
        let cache_dir = tempfile::tempdir()?;
        let mut config = Config::test_config_with_kind(StorageKind::Memory)?;
        config.local_archive_cache_path = cache_dir.path().to_path_buf();
        let cache = Cache::new(&config);
        Ok(TestEnv {
            _cache_dir: cache_dir,
            _config: config,
            cache,
        })
    }

    #[tokio::test]
    async fn index_create_save_load_sqlite() -> Result<()> {
        let tf = create_test_archive(1).await?;

        let tempfile = tempfile::NamedTempFile::new()?.into_temp_path();
        create(tf, &tempfile).await?;

        let fi = find_in_file(&tempfile, "testfile0").await?.unwrap();

        assert_eq!(fi.range, FileRange::new(39, 459));
        assert_eq!(fi.compression, CompressionAlgorithm::Bzip2);

        assert!(find_in_file(&tempfile, "some_other_file",).await?.is_none());
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
        let cache = test_cache()?;

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
        let cache = test_cache()?;
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

        Ok(())
    }

    #[tokio::test]
    async fn find_downloads_when_local_cache_missing() -> Result<()> {
        let cache = test_cache()?;
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
        let cache = test_cache()?;
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
        let cache = test_cache()?;
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
        assert_eq!(downloader.download_count(&remote_index_path), 2);

        Ok(())
    }

    #[tokio::test]
    async fn purge_removes_index_wal_and_shm() -> Result<()> {
        let cache = test_cache()?;
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
        let cache = test_cache()?;
        cache.purge("missing.zip", Some(BuildId(7))).await?;
        cache.purge("missing.zip", Some(BuildId(7))).await?;

        Ok(())
    }

    #[tokio::test]
    async fn download_archive_index_overwrites_existing_file() -> Result<()> {
        let cache = test_cache()?;
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
        let cache = test_cache()?;
        let cache = Arc::new(cache.cache);
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
