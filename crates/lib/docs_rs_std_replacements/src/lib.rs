//! Cached standard-library alternatives to third-party crates.
mod config;
mod models;
mod real;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use config::Config;
pub use models::{ReplacementDetails, ReplacementMap};
pub use real::StdReplacementsImpl;

use anyhow::Result;
use async_trait::async_trait;
use docs_rs_types::KrateName;
use std::sync::Arc;

/// Looks up standard-library alternatives independently of their data source.
#[async_trait]
pub trait StdReplacementsProvider: Send + Sync {
    /// Return the alternative for a crate, or `None` when none is known.
    async fn get(&self, name: &KrateName) -> Result<Option<Arc<ReplacementDetails>>>;
}

/// A shared replacement client, backed by either the HTTP implementation or a test mock.
pub type StdReplacements = Arc<dyn StdReplacementsProvider>;
