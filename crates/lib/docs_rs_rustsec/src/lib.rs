//! Access to a periodically refreshed local RustSec advisory database.
//!
//! ```no_run
//! # async fn example() -> anyhow::Result<()> {
//! use docs_rs_rustsec::{Config, RustsecClient};
//! use docs_rs_types::Version;
//!
//! let client = RustsecClient::from_config(&Config::builder().build())?;
//! if let Some(database) = client.database() {
//!     let advisory = database.find_unmaintained(&"owned-alloc".parse()?, &Version::new(1, 0, 0));
//! }
//! # Ok(())
//! # }
//! ```
mod api;
mod config;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use api::{RustsecClient, RustsecDatabase};
pub use config::{Config, ConfigBuilder};
