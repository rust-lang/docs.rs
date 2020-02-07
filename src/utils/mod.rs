//! Various utilities for cratesfyi

pub(crate) use self::cargo_metadata::{CargoMetadata, Package as MetadataPackage};
pub use self::copy::{copy_dir, copy_doc_dir};
pub use self::daemon::start_daemon;
pub use self::github_updater::github_updater;
pub use self::html::extract_head_and_body;
pub use self::queue::add_crate_to_queue;
pub use self::release_activity_updater::update_release_activity;
pub use self::rustc_version::{command_result, get_current_versions, parse_rustc_version};

#[cfg(test)]
pub(crate) use self::cargo_metadata::{Dependency, Target};

mod cargo_metadata;
mod copy;
mod daemon;
mod github_updater;
mod html;
mod pubsubhubbub;
mod queue;
mod release_activity_updater;
mod rustc_version;
