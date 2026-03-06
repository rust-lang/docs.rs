use std::ops::RangeInclusive;
use strum::EnumString;

pub type FileRange = RangeInclusive<u64>;

#[derive(Debug, Copy, Clone, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum StorageKind {
    #[cfg(any(test, feature = "testing"))]
    Memory,
    S3,
}

impl Default for StorageKind {
    fn default() -> Self {
        #[cfg(any(test, feature = "testing"))]
        return StorageKind::Memory;
        #[cfg(not(any(test, feature = "testing")))]
        return StorageKind::S3;
    }
}
