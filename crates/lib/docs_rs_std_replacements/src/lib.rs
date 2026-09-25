//! Standard-library alternatives to third-party crates, refreshed in the background.
mod api;
mod config;
mod models;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use api::StdReplacements;
pub use config::{Config, ConfigBuilder};
pub use models::{ReplacementDetails, ReplacementMap};
