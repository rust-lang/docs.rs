//! Access to crate metadata split between the sparse registry index and its HTTP API.

mod api;
mod config;
mod error;
mod models;
#[cfg(any(test, feature = "testing"))]
/// Test utilities for supplying a local, fully mocked registry.
pub mod testing;

pub use api::RegistryApi;
pub use config::Config;
pub use error::Error;
pub use models::{
    CrateData, CrateOwner, OwnerKind, ReleaseData, Search, SearchCursor, SearchQuery, SearchSort,
};
