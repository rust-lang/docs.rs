//! Various utilities for cratesfyi


pub(crate) use self::copy::copy_doc_dir;
pub use self::github_updater::github_updater;
pub use self::release_activity_updater::update_release_activity;
pub use self::daemon::start_daemon;
pub(crate) use self::rustc_version::parse_rustc_version;
pub use self::html::extract_head_and_body;
pub use self::queue::add_crate_to_queue;
pub(crate) use self::cargo_metadata::{CargoMetadata, Package as MetadataPackage};

#[cfg(test)]
pub(crate) use self::cargo_metadata::{Dependency, Target};

mod cargo_metadata;
mod github_updater;
mod copy;
mod release_activity_updater;
mod daemon;
mod pubsubhubbub;
mod rustc_version;
mod html;
mod queue;
