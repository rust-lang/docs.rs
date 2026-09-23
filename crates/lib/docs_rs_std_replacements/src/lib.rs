//! Cached standard-library alternatives to third-party crates.
mod api;
mod config;
mod models;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use api::StdReplacements;
pub use config::{Config, ConfigBuilder};
pub use models::{ReplacementDetails, ReplacementMap};
