use crate::error::Result;
use crate::storage::{compression::CompressionAlgorithm, FileRange};
use anyhow::{bail, Context as _};
use memmap::MmapOptions;
use serde::de::DeserializeSeed;
use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::{fs, io};

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

pub(crate) fn create<R: io::Read + io::Seek, W: io::Write>(
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
                range: FileRange::new(zf.data_start(), zf.data_start() + zf.compressed_size() - 1),
                compression: match zf.compression() {
                    zip::CompressionMethod::Bzip2 => CompressionAlgorithm::Bzip2,
                    c => bail!("unsupported compression algorithm {} in zip-file", c),
                },
            },
        );
    }

    serde_cbor::to_writer(writer, &Index { files }).context("serialization error")
}

pub(crate) fn find_in_slice(bytes: &[u8], search_for: &str) -> Result<Option<FileInfo>> {
    let mut deserializer = serde_cbor::Deserializer::from_slice(bytes);

    /// This visitor will just find the `files` element in the top-level map.
    /// Then it will call the `FindFileVisitor` that should find the actual
    /// FileInfo for the path we are searching for.
    struct FindFileListVisitor {
        search_for: String,
    }

    impl FindFileListVisitor {
        pub fn new(path: String) -> Self {
            FindFileListVisitor { search_for: path }
        }
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

            impl FindFileVisitor {
                pub fn new(search_for: String) -> Self {
                    FindFileVisitor { search_for }
                }
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
                    return map.next_value_seed(FindFileVisitor::new(self.search_for));
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

    Ok(FindFileListVisitor::new(search_for.to_string()).deserialize(&mut deserializer)?)
}

pub(crate) fn find_in_file<P: AsRef<Path>>(
    archive_index_path: P,
    search_for: &str,
) -> Result<Option<FileInfo>> {
    let file = fs::File::open(archive_index_path).context("could not open file")?;
    let mmap = unsafe {
        MmapOptions::new()
            .map(&file)
            .context("could not create memory map")?
    };

    find_in_slice(&mmap, search_for)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::FileOptions;

    #[test]
    fn index_create_save_load() {
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

        let mut buf = Vec::new();
        create(&mut tf, &mut buf).unwrap();

        let fi = find_in_slice(&buf, "testfile1").unwrap().unwrap();
        assert_eq!(fi.range, FileRange::new(39, 459));
        assert_eq!(fi.compression, CompressionAlgorithm::Bzip2);

        assert!(find_in_slice(&buf, "some_other_file").unwrap().is_none());
    }
}
