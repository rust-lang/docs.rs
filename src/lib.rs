//! [Docs.rs](https://docs.rs) (formerly cratesfyi) is an open source project to host
//! documentation of crates for the Rust Programming Language.
#![allow(clippy::cognitive_complexity)]

pub use self::build_queue::BuildQueue;

pub use self::config::Config;
pub use self::context::Context;
pub use self::docbuilder::options::DocBuilderOptions;
pub use self::docbuilder::DocBuilder;
pub use self::docbuilder::RustwideBuilder;
pub use self::metrics::Metrics;
pub use self::storage::Storage;
pub use self::web::Server;

mod build_queue;
mod config;
mod context;
pub mod db;
mod docbuilder;
mod error;
pub mod index;
mod metrics;
pub mod storage;
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

/// Version string generated at build time contains last git
/// commit hash and build date
pub const BUILD_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " ",
    include_str!(concat!(env!("OUT_DIR"), "/git_version"))
);
