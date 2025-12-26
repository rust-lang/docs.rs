mod config;
mod metrics;
pub mod priority;
mod queue;
mod types;

pub use config::Config;
pub use queue::{blocking::BuildQueue, non_blocking::AsyncBuildQueue};
pub use types::{BuildPackageSummary, QueuedCrate};

pub const PRIORITY_DEFAULT: i32 = 0;
/// Used for workspaces to avoid blocking the queue (done through the cratesfyi CLI, not used in code)
#[allow(dead_code)]
pub const PRIORITY_DEPRIORITIZED: i32 = 1;
/// Rebuilds triggered from crates.io, see issue #2442
pub const PRIORITY_MANUAL_FROM_CRATES_IO: i32 = 5;
/// Used for rebuilds queued through cratesfyi for crate versions failed due to a broken Rustdoc nightly version.
/// Note: a broken rustdoc version does not necessarily imply a failed build.
pub const PRIORITY_BROKEN_RUSTDOC: i32 = 10;
/// Used by the synchronize cratesfyi command when queueing builds that are in the crates.io index but not in the database.
pub const PRIORITY_CONSISTENCY_CHECK: i32 = 15;
/// The static priority for background rebuilds, used when queueing rebuilds, and when rendering them collapsed in the UI.
pub const PRIORITY_CONTINUOUS: i32 = 20;
