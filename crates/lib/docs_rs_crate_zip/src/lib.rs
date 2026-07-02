//! Library to read the crates.io source archives (manifest & zip), and
//! fetch single files from the remote archives.
//!
//! Archives are created here:
//! https://github.com/rust-lang/crates.io/blob/5274087feb193ee490e9a6bbdf2e18e74e9ddaeb/crates/crates_io_crate_zip/src/lib.rs
//! Also we copied the manifest structs from there.

mod source_archive;

pub use source_archive::{FileEntry, Manifest, SourceArchive};
