//! [Docs.rs](https://docs.rs) (formerly cratesfyi) is an open source project to host
//! documentation of crates for the Rust Programming Language.
#![allow(
    clippy::cognitive_complexity,
    // TODO: `AxumNope::Redirect(EscapedURI, CachePolicy)` is too big.
    clippy::result_large_err,
)]

pub use self::build_queue::{
    AsyncBuildQueue, BuildQueue, queue_rebuilds, queue_rebuilds_faulty_rustdoc,
};
pub use self::config::Config;
pub use self::context::Context;
pub use self::docbuilder::PackageKind;
pub use self::docbuilder::{BuildPackageSummary, RustwideBuilder};
pub use self::index::Index;
pub use self::registry_api::RegistryApi;
pub use self::storage::{AsyncStorage, Storage};
pub use self::web::start_web_server;

pub use docs_rs_utils::{
    APP_USER_AGENT, BUILD_VERSION, DEFAULT_MAX_TARGETS, RUSTDOC_STATIC_STORAGE_PREFIX,
};
pub use font_awesome_as_a_crate::icons;

mod build_queue;
pub mod cdn;
mod config;
mod context;
pub mod db;
mod docbuilder;
mod error;
pub mod index;
pub mod metrics;
mod registry_api;
pub mod repositories;
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
