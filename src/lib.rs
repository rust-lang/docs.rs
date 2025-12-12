//! [Docs.rs](https://docs.rs) (formerly cratesfyi) is an open source project to host
//! documentation of crates for the Rust Programming Language.
#![allow(clippy::cognitive_complexity)]

pub use self::config::Config;
pub use self::context::Context;
pub use self::docbuilder::PackageKind;
pub use self::docbuilder::{BuildPackageSummary, RustwideBuilder};
pub use self::web::start_web_server;

pub use font_awesome_as_a_crate::icons;

mod config;
mod context;
pub mod db;
mod docbuilder;
pub mod metrics;
#[cfg(test)]
mod test;
pub mod utils;
mod web;
