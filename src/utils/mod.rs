//! Various utilities for cratesfyi


pub use self::build_doc::{build_doc, get_package, source_path, update_sources};
pub use self::copy::{copy_dir, copy_doc_dir};
pub use self::github_updater::github_updater;
pub use self::release_activity_updater::update_release_activity;
pub use self::daemon::start_daemon;
pub use self::rustc_version::{parse_rustc_version, get_current_versions, command_result};
pub use self::html::extract_head_and_body;

mod github_updater;
mod build_doc;
mod copy;
mod release_activity_updater;
mod daemon;
mod pubsubhubbub;
mod rustc_version;
mod html;
mod rustup;
