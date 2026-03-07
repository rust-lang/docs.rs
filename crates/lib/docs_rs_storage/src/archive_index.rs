use crate::types::FileRange;
use anyhow::{Context as _, Result, anyhow, bail};
use docs_rs_types::CompressionAlgorithm;
use docs_rs_utils::spawn_blocking;
use sqlx::{Acquire as _, ConnectOptions as _, QueryBuilder, Row as _, Sqlite};
use std::path::Path;
use tokio::{
    fs,
    io::{AsyncRead, AsyncSeek},
    sync::mpsc,
};
use tokio_util::io::SyncIoBridge;
use tracing::instrument;

pub(crate) const ARCHIVE_INDEX_FILE_EXTENSION: &str = "index";

#[derive(PartialEq, Eq, Debug)]
pub(crate) struct FileInfo {
    range: FileRange,
    compression: CompressionAlgorithm,
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
    if path.exists() {
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
        .pragma("synchronous", "off") // not needed for readonly db
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
pub(crate) async fn create<
    R: AsyncRead + AsyncSeek + Unpin + Send + 'static,
    P: AsRef<Path> + std::fmt::Debug,
>(
    zipfile: R,
    destination: P,
) -> Result<R> {
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
pub(crate) async fn find_in_file<P: AsRef<Path> + std::fmt::Debug>(
    archive_index_path: P,
    search_for: &str,
) -> Result<Option<FileInfo>> {
    let mut conn = sqlite_open(archive_index_path).await?;

    find_in_sqlite_index(&mut conn, search_for).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    async fn create_test_archive(file_count: u32) -> Result<fs::File> {
        spawn_blocking(move || {
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
}
