//! [Docs.rs](https://docs.rs) (formerly cratesfyi) is an open source project to host
//! documentation of crates for the Rust Programming Language.
#![allow(clippy::cognitive_complexity)]

pub use self::config::Config;
pub use self::context::Context;
pub use self::docbuilder::PackageKind;
pub use self::docbuilder::{BuildPackageSummary, RustwideBuilder};
pub use self::registry_api::RegistryApi;
pub use self::web::start_web_server;

pub use font_awesome_as_a_crate::icons;

pub mod cdn;
mod config;
mod context;
pub mod db;
mod docbuilder;
mod error;
pub mod metrics;
mod registry_api;
#[cfg(test)]
mod test;
pub mod utils;
mod web;

use web::page::GlobalAlert;

// Warning message shown in the navigation bar of every page. Set to `None` to hide it.
pub(crate) static GLOBAL_ALERT: Option<GlobalAlert> = None;
/*
pub(crate) static GLOBAL_ALERT: Option<GlobalAlert> = Some(GlobalAlert {
    url: "https://blog.rust-lang.org/2019/09/18/upcoming-docsrs-changes.html",
    text: "Upcoming docs.rs breaking changes!",
    css_class: "error",
    fa_icon: "exclamation-triangle",
});
*/

/// Where rustdoc's static files are stored in S3.
/// Since the prefix starts with `/`, it needs to be referenced with a double slash in
/// API & AWS CLI.
/// Example:
/// `s3://rust-docs-rs//rustdoc-static/something.css`
pub const RUSTDOC_STATIC_STORAGE_PREFIX: &str = "/rustdoc-static/";

/// Maximum number of targets allowed for a crate to be documented on.
pub const DEFAULT_MAX_TARGETS: usize = 10;
