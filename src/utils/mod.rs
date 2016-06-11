//! Various utilities for cratesfyi


pub use self::build_doc::{build_doc, get_package, source_path, update_sources};
pub use self::copy::{copy_dir, copy_doc_dir};
pub use self::github_updater::github_updater;

mod github_updater;
mod build_doc;
mod copy;
