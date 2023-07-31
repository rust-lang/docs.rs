use crate::error::Result;
use crate::storage::{compression::CompressionAlgorithm, FileRange};
use anyhow::{bail, Context as _};
use rusqlite::{Connection, OptionalExtension};
use std::{fs, io, path::Path};

use super::sqlite_pool::SqliteConnectionPool;

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

/// create an archive index based on a zipfile.
///
/// Will delete the destination file if it already exists.
pub(crate) fn create<R: io::Read + io::Seek, P: AsRef<Path>>(
    zipfile: &mut R,
    destination: P,
) -> Result<()> {
    let destination = destination.as_ref();
    if destination.exists() {
        fs::remove_file(destination)?;
    }

    let conn = rusqlite::Connection::open(destination)?;
    conn.execute("PRAGMA synchronous = FULL", ())?;
    conn.execute("BEGIN", ())?;
    conn.execute(
        "
            CREATE TABLE files (
                id INTEGER PRIMARY KEY,
                path TEXT UNIQUE,
                start INTEGER,
                end INTEGER,
                compression INTEGER
            );
            ",
        (),
    )?;

    let mut archive = zip::ZipArchive::new(zipfile)?;
    let compression_bzip = CompressionAlgorithm::Bzip2 as i32;

    for i in 0..archive.len() {
        let zf = archive.by_index(i)?;

        conn.execute(
            "INSERT INTO files (path, start, end, compression) VALUES (?, ?, ?, ?)",
            (
                zf.name(),
                zf.data_start(),
                zf.data_start() + zf.compressed_size() - 1,
                match zf.compression() {
                    zip::CompressionMethod::Bzip2 => compression_bzip,
                    c => bail!("unsupported compression algorithm {} in zip-file", c),
                },
            ),
        )?;
    }
    conn.execute("CREATE INDEX idx_files_path ON files (path);", ())?;
    conn.execute("END", ())?;
    conn.execute("VACUUM", ())?;
    Ok(())
}

fn find_in_sqlite_index(conn: &Connection, search_for: &str) -> Result<Option<FileInfo>> {
    let mut stmt = conn.prepare(
        "
        SELECT start, end, compression 
        FROM files 
        WHERE path = ?
        ",
    )?;

    stmt.query_row((search_for,), |row| {
        let compression: i32 = row.get(2)?;

        Ok(FileInfo {
            range: row.get(0)?..=row.get(1)?,
            compression: compression.try_into().map_err(|value| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Integer,
                    format!("invalid compression algorithm '{}' in database", value).into(),
                )
            })?,
        })
    })
    .optional()
    .context("error fetching SQLite data")
}

pub(crate) fn find_in_file<P: AsRef<Path>>(
    archive_index_path: P,
    search_for: &str,
    pool: &SqliteConnectionPool,
) -> Result<Option<FileInfo>> {
    pool.with_connection(archive_index_path, |connection| {
        find_in_sqlite_index(connection, search_for)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::FileOptions;

    fn create_test_archive() -> fs::File {
        let mut tf = tempfile::tempfile().unwrap();

        let objectcontent: Vec<u8> = (0..255).collect();

        let mut archive = zip::ZipWriter::new(tf);
        archive
            .start_file(
                "testfile1",
                FileOptions::default().compression_method(zip::CompressionMethod::Bzip2),
            )
            .unwrap();
        archive.write_all(&objectcontent).unwrap();
        tf = archive.finish().unwrap();
        tf
    }

    #[test]
    fn index_create_save_load_sqlite() {
        let mut tf = create_test_archive();

        let tempfile = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        create(&mut tf, &tempfile).unwrap();

        let fi = find_in_file(&tempfile, "testfile1", &SqliteConnectionPool::default())
            .unwrap()
            .unwrap();

        assert_eq!(fi.range, FileRange::new(39, 459));
        assert_eq!(fi.compression, CompressionAlgorithm::Bzip2);

        assert!(find_in_file(
            &tempfile,
            "some_other_file",
            &SqliteConnectionPool::default(),
        )
        .unwrap()
        .is_none());
    }
}
