mod archive_index;
mod backends;
mod blob;
pub mod compression;
mod config;
pub(crate) mod errors;
mod file;
mod metrics;
pub(crate) mod storage;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub(crate) mod types;
pub(crate) mod utils;

pub use blob::{Blob, BlobUpload, StreamingBlob};
pub use compression::{compress, compress_async, decompress};
pub use config::Config;
pub use errors::{PathNotFoundError, SizeLimitReached};
pub use file::FileEntry;
pub use file::file_list_to_json;
pub use storage::blocking::Storage;
pub use storage::non_blocking::AsyncStorage;
pub use types::StorageKind;
pub use utils::{
    file_list::get_file_list,
    storage_path::{rustdoc_archive_path, rustdoc_json_path, source_archive_path},
};
