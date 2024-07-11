use crate::error::Result;
use crate::storage::{compression::CompressionAlgorithm, FileRange};
use anyhow::{bail, Context as _};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{fs, io, path::Path};
use tracing::instrument;

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
#[instrument(skip(zipfile))]
pub(crate) fn create<R: io::Read + io::Seek, P: AsRef<Path> + std::fmt::Debug>(
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

#[instrument]
pub(crate) fn find_in_file<P: AsRef<Path> + std::fmt::Debug>(
    archive_index_path: P,
    search_for: &str,
) -> Result<Option<FileInfo>> {
    let connection = Connection::open_with_flags(
        archive_index_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    find_in_sqlite_index(&connection, search_for)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn create_test_archive(file_count: u32) -> fs::File {
        let mut tf = tempfile::tempfile().unwrap();

        let objectcontent: Vec<u8> = (0..255).collect();

        let mut archive = zip::ZipWriter::new(tf);
        for i in 0..file_count {
            archive
                .start_file(
                    format!("testfile{i}"),
                    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Bzip2),
                )
                .unwrap();
            archive.write_all(&objectcontent).unwrap();
        }
        tf = archive.finish().unwrap();
        tf
    }

    #[test]
    fn index_create_save_load_sqlite() {
        let mut tf = create_test_archive(1);

        let tempfile = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        create(&mut tf, &tempfile).unwrap();

        let fi = find_in_file(&tempfile, "testfile0").unwrap().unwrap();

        assert_eq!(fi.range, FileRange::new(39, 459));
        assert_eq!(fi.compression, CompressionAlgorithm::Bzip2);

        assert!(find_in_file(&tempfile, "some_other_file",)
            .unwrap()
            .is_none());
    }

    #[test]
    fn archive_with_more_than_65k_files() {
        let mut tf = create_test_archive(100_000);

        let tempfile = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        create(&mut tf, &tempfile).unwrap();

        let connection = Connection::open_with_flags(
            tempfile,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        let mut stmt = connection.prepare("SELECT count(*) FROM files").unwrap();

        let count = stmt
            .query_row([], |row| Ok(row.get::<_, usize>(0)))
            .unwrap()
            .unwrap();
        assert_eq!(count, 100_000);
    }
}
