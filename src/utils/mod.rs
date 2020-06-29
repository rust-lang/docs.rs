//! Various utilities for cratesfyi

pub(crate) use self::cargo_metadata::{CargoMetadata, Package as MetadataPackage};
pub(crate) use self::copy::copy_doc_dir;
pub use self::daemon::start_daemon;
pub use self::github_updater::github_updater;
pub use self::html::extract_head_and_body;
pub use self::queue::{get_crate_priority, remove_crate_priority, set_crate_priority};
pub use self::release_activity_updater::update_release_activity;
pub(crate) use self::rustc_version::parse_rustc_version;

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
pub(crate) mod sized_buffer;
