use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    /// One entry per file in the zip, sorted alphabetically by path.
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileEntry {
    /// Realtive path (without the leading `{name}-{version}/` component of
    /// the tarball).
    pub path: String,
    /// Byte offset in the zip where this entry's compressed payload begins.
    pub data_offset: u64,
    /// Length of the compressed contents in bytes.
    pub compressed_size: u64,
    /// Length of the uncompressed contents in bytes.
    pub uncompressed_size: u64,
    /// How the payload is compressed: `"deflate"` or `"store"`.
    pub compression: String,
    /// Lowercase hex sha256 of the uncompressed contents.
    pub sha256: String,
}
