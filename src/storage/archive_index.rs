use crate::error::Result;
use crate::storage::{compression::CompressionAlgorithm, FileRange};
use anyhow::{bail, Context as _};
use memmap2::MmapOptions;
use rusqlite::{Connection, OptionalExtension};
use serde::de::DeserializeSeed;
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::{collections::HashMap, fmt, fs, fs::File, io, io::Read, path::Path};

use super::sqlite_pool::SqliteConnectionPool;

static SQLITE_FILE_HEADER: &[u8] = b"SQLite format 3\0";

#[derive(Deserialize, Serialize)]
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

#[derive(Serialize)]
struct Index {
    files: HashMap<String, FileInfo>,
}

/// create an archive index based on a zipfile.
///
/// Will delete the destination file if it already exists.
pub(crate) fn create<R: io::Read + io::Seek, P: AsRef<Path>>(
    zipfile: &mut R,
    destination: P,
) -> Result<()> {
    if destination.as_ref().exists() {
        fs::remove_file(&destination)?;
    }

    let mut archive = zip::ZipArchive::new(zipfile)?;

    let conn = rusqlite::Connection::open(&destination)?;
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

    for i in 0..archive.len() {
        let zf = archive.by_index(i)?;

        let compression_bzip = CompressionAlgorithm::Bzip2 as i32;

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

fn find_in_slice(bytes: &[u8], search_for: &str) -> Result<Option<FileInfo>> {
    let mut deserializer = serde_cbor::Deserializer::from_slice(bytes);

    /// This visitor will just find the `files` element in the top-level map.
    /// Then it will call the `FindFileVisitor` that should find the actual
    /// FileInfo for the path we are searching for.
    struct FindFileListVisitor {
        search_for: String,
    }

    impl<'de> Visitor<'de> for FindFileListVisitor {
        type Value = Option<FileInfo>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(formatter, "a map with a 'files' key")
        }

        fn visit_map<V>(self, mut map: V) -> Result<Self::Value, V::Error>
        where
            V: MapAccess<'de>,
        {
            /// This visitor will walk the full `files` map and search for
            /// the path we want to have.
            /// Return value is just the `FileInfo` we want to have, or
            /// `None`.
            struct FindFileVisitor {
                search_for: String,
            }

            impl<'de> DeserializeSeed<'de> for FindFileVisitor {
                type Value = Option<FileInfo>;
                fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
                where
                    D: Deserializer<'de>,
                {
                    deserializer.deserialize_map(self)
                }
            }

            impl<'de> Visitor<'de> for FindFileVisitor {
                type Value = Option<FileInfo>;
                fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                    write!(
                        formatter,
                        "a map with path => FileInfo, searching for path {:?}",
                        self.search_for
                    )
                }
                fn visit_map<V>(self, mut map: V) -> Result<Self::Value, V::Error>
                where
                    V: MapAccess<'de>,
                {
                    while let Some(key) = map.next_key::<&str>()? {
                        if key == self.search_for {
                            let value = map.next_value::<FileInfo>()?;
                            // skip over the rest of the data without really parsing it.
                            // If we don't do this the serde_cbor deserializer fails because not
                            // the whole map is consumed.
                            while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
                            return Ok(Some(value));
                        } else {
                            // skip parsing the FileInfo structure when the key doesn't match.
                            map.next_value::<IgnoredAny>()?;
                        }
                    }

                    Ok(None)
                }
            }

            while let Some(key) = map.next_key::<&str>()? {
                if key == "files" {
                    return map.next_value_seed(FindFileVisitor {
                        search_for: self.search_for,
                    });
                }
            }

            Ok(None)
        }
    }

    impl<'de> DeserializeSeed<'de> for FindFileListVisitor {
        type Value = Option<FileInfo>;

        fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserializer.deserialize_map(self)
        }
    }

    Ok(FindFileListVisitor {
        search_for: search_for.to_string(),
    }
    .deserialize(&mut deserializer)?)
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
            compression: compression.try_into().expect("invalid compression value"),
        })
    })
    .optional()
    .context("error fetching SQLite data")
}

/// quick check if a file is a SQLite file.
///
/// Helpful for the transition phase where an archive-index might be
/// old (CBOR) or new (SQLite) format.
///
/// See
/// https://raw.githubusercontent.com/rusqlite/rusqlite/master/libsqlite3-sys/sqlite3/sqlite3.c
/// and
/// https://en.wikipedia.org/wiki/SQLite (-> _Magic number_)
/// ```ignore
/// > FORMAT DETAILS
/// > OFFSET   SIZE    DESCRIPTION
/// >    0      16     Header string: "SQLite format 3\000"
/// > [...]
fn is_sqlite_file<P: AsRef<Path>>(archive_index_path: P) -> Result<bool> {
    let mut f = File::open(archive_index_path)?;

    let mut buffer = [0; 16];
    match f.read_exact(&mut buffer) {
        Ok(()) => Ok(buffer == SQLITE_FILE_HEADER),
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(err) => Err(err.into()),
    }
}

pub(crate) fn find_in_file<P: AsRef<Path>>(
    archive_index_path: P,
    search_for: &str,
    pool: &SqliteConnectionPool,
) -> Result<Option<FileInfo>> {
    if is_sqlite_file(&archive_index_path)? {
        pool.with_connection(archive_index_path, |connection| {
            find_in_sqlite_index(connection, search_for)
        })
    } else {
        let file = fs::File::open(archive_index_path).context("could not open file")?;
        let mmap = unsafe {
            MmapOptions::new()
                .map(&file)
                .context("could not create memory map")?
        };

        find_in_slice(&mmap, search_for)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::FileOptions;

    /// legacy archive index creation, only for testing that reading them still works
    fn create_cbor_index<R: io::Read + io::Seek, W: io::Write>(
        zipfile: &mut R,
        writer: &mut W,
    ) -> Result<()> {
        let mut archive = zip::ZipArchive::new(zipfile)?;

        // get file locations
        let mut files: HashMap<String, FileInfo> = HashMap::with_capacity(archive.len());
        for i in 0..archive.len() {
            let zf = archive.by_index(i)?;

            files.insert(
                zf.name().to_string(),
                FileInfo {
                    range: FileRange::new(
                        zf.data_start(),
                        zf.data_start() + zf.compressed_size() - 1,
                    ),
                    compression: match zf.compression() {
                        zip::CompressionMethod::Bzip2 => CompressionAlgorithm::Bzip2,
                        c => bail!("unsupported compression algorithm {} in zip-file", c),
                    },
                },
            );
        }

        serde_cbor::to_writer(writer, &Index { files }).context("serialization error")
    }

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
    fn index_create_save_load_cbor_direct() {
        let mut tf = create_test_archive();
        let mut buf = Vec::new();
        create_cbor_index(&mut tf, &mut buf).unwrap();

        let fi = find_in_slice(&buf, "testfile1").unwrap().unwrap();
        assert_eq!(fi.range, FileRange::new(39, 459));
        assert_eq!(fi.compression, CompressionAlgorithm::Bzip2);

        assert!(find_in_slice(&buf, "some_other_file").unwrap().is_none());
    }

    #[test]
    fn index_create_save_load_cbor_as_fallback() {
        let mut tf = create_test_archive();
        let mut cbor_buf = Vec::new();
        create_cbor_index(&mut tf, &mut cbor_buf).unwrap();
        let mut cbor_index_file = tempfile::NamedTempFile::new().unwrap();
        io::copy(&mut &cbor_buf[..], &mut cbor_index_file).unwrap();

        assert!(!is_sqlite_file(&cbor_index_file).unwrap());

        let fi = find_in_file(
            cbor_index_file.path(),
            "testfile1",
            &SqliteConnectionPool::default(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(fi.range, FileRange::new(39, 459));
        assert_eq!(fi.compression, CompressionAlgorithm::Bzip2);

        assert!(find_in_file(
            cbor_index_file.path(),
            "some_other_file",
            &SqliteConnectionPool::default(),
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn index_create_save_load_sqlite() {
        let mut tf = create_test_archive();

        let tempfile = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        create(&mut tf, &tempfile).unwrap();
        assert!(is_sqlite_file(&tempfile).unwrap());

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

    #[test]
    fn is_sqlite_file_empty() {
        let tempfile = tempfile::NamedTempFile::new().unwrap().into_temp_path();
        assert!(!is_sqlite_file(tempfile).unwrap());
    }

    #[test]
    fn is_sqlite_file_other_content() {
        let mut tempfile = tempfile::NamedTempFile::new().unwrap();
        tempfile.write_all(b"some_bytes").unwrap();
        assert!(!is_sqlite_file(tempfile.path()).unwrap());
    }

    #[test]
    fn is_sqlite_file_specific_headers() {
        let mut tempfile = tempfile::NamedTempFile::new().unwrap();
        tempfile.write_all(SQLITE_FILE_HEADER).unwrap();
        assert!(is_sqlite_file(tempfile.path()).unwrap());
    }
}
