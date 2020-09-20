//! Various utilities for docs.rs

pub(crate) use self::cargo_metadata::{CargoMetadata, Dependency, Package as MetadataPackage};
pub(crate) use self::copy::copy_doc_dir;
pub use self::daemon::start_daemon;
pub use self::github_updater::GithubUpdater;
pub(crate) use self::html::rewrite_lol;
pub use self::queue::{get_crate_priority, remove_crate_priority, set_crate_priority};
pub use self::queue_builder::queue_builder;
pub use self::release_activity_updater::update_release_activity;
pub(crate) use self::rustc_version::parse_rustc_version;

#[cfg(test)]
pub(crate) use self::cargo_metadata::Target;

mod cargo_metadata;
pub mod consistency;
mod copy;
mod daemon;
mod github_updater;
mod html;
mod pubsubhubbub;
mod queue;
mod queue_builder;
mod release_activity_updater;
mod rustc_version;
pub(crate) mod sized_buffer;
