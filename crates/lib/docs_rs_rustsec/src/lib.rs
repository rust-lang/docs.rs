//! HTTP access to RustSec's per-package OSV advisory feeds.
//!
//! ```no_run
//! # async fn example() -> anyhow::Result<()> {
//! use docs_rs_rustsec::{Config, RustsecClient};
//!
//! let client = RustsecClient::from_config(&Config::builder().build())?;
//! let advisory = client.find_unmaintained(&"owned-alloc".parse()?).await?;
//! # Ok(())
//! # }
//! ```
mod api;
mod config;
mod models;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use api::RustsecClient;
pub use config::{Config, ConfigBuilder};
pub use models::advisory::{Id, Informational};
pub use models::osv::{OsvAdvisory, OsvAffected};
