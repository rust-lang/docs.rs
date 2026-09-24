use docs_rs_types::{ByteSize, CompressionAlgorithm};

/// When we create an zip archive for source or rustdoc files,
/// we collect some statistics we need.
pub struct ArchiveStatistics {
    /// used compression algorithm
    pub alg: CompressionAlgorithm,
    /// original size of all files
    pub original_size: ByteSize,
    /// file count in the archive.
    pub file_count: u64,
}

impl ArchiveStatistics {
    pub fn new(alg: CompressionAlgorithm) -> Self {
        Self {
            alg,
            original_size: ByteSize::b(0),
            file_count: 0,
        }
    }
}
